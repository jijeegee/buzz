import 'package:flutter/foundation.dart';

import 'feed_item.dart';

/// Inbox filters, mirroring desktop's `InboxFilter` in
/// `desktop/src/features/home/lib/inbox.ts`.
enum InboxFilter {
  all,
  mention,
  thread,
  conversations,
  needsAction,
  activity,
  agentActivity,
  reminders,
  drafts,
}

/// Category ordering used to pick the label for a grouped conversation.
/// Mirrors desktop's `categoryPriority`.
int categoryPriority(String category) {
  return switch (category) {
    'needs_action' => 0,
    'mention' => 1,
    'agent_activity' => 2,
    _ => 3,
  };
}

/// Thread reference from NIP-10 tags — desktop's `getThreadReference`.
({String? parentId, String? rootId}) threadReferenceOf(
  List<List<String>> tags,
) {
  List<String>? rootTag;
  List<String>? replyTag;
  for (final tag in tags) {
    if (tag.length >= 4 && tag[0] == 'e') {
      if (tag[3] == 'root') rootTag = tag;
      if (tag[3] == 'reply') replyTag = tag;
    }
  }
  if (replyTag == null) return (parentId: null, rootId: null);
  final parentId = replyTag[1];
  return (parentId: parentId, rootId: rootTag?[1] ?? parentId);
}

/// Broadcast replies surface at the channel top level — desktop's
/// `isBroadcastReply`.
bool isBroadcastReply(List<List<String>> tags) {
  return tags.any(
    (tag) => tag.length >= 2 && tag[0] == 'broadcast' && tag[1] == '1',
  );
}

/// A thread reply that is not broadcast — desktop's `isThreadReply`.
bool isThreadReply(List<List<String>> tags) {
  return threadReferenceOf(tags).parentId != null && !isBroadcastReply(tags);
}

/// Stable conversation identity: `rootId ?? parentId ?? event.id`, except
/// ordinary (non-thread) DM messages, which group by DM channel identity so
/// one DM conversation renders as one row. Mirrors desktop's
/// `getInboxConversationId`.
String inboxConversationId(
  List<List<String>> tags,
  String eventId, {
  String? dmChannelId,
}) {
  final thread = threadReferenceOf(tags);
  final threadId = thread.rootId ?? thread.parentId;
  if (threadId != null) return threadId;
  if (dmChannelId != null) return 'dm:$dmChannelId';
  return eventId;
}

/// One conversation-grouped inbox row. Mirrors desktop's `InboxItem`.
@immutable
class InboxItem {
  /// Stable conversation identity for the thread group.
  final String conversationId;

  /// Representative (latest) event id.
  final String id;

  /// Representative (latest) feed item.
  final FeedItem item;

  /// All events grouped into this conversation, unordered.
  final List<FeedItem> groupItems;

  /// Distinct categories present in the group, priority-sorted.
  final List<String> categories;

  final bool isActionRequired;
  final int latestActivityAt;

  const InboxItem({
    required this.conversationId,
    required this.id,
    required this.item,
    required this.groupItems,
    required this.categories,
    required this.isActionRequired,
    required this.latestActivityAt,
  });

  /// Root id if any grouped event carries thread-reply tags.
  String? get threadRootId {
    for (final candidate in [item, ...groupItems]) {
      if (isThreadReply(candidate.tags)) {
        return threadReferenceOf(candidate.tags).rootId;
      }
    }
    return null;
  }

  /// The event the row should deep-link to: the oldest event in the group
  /// newer than [readAt] (oldest unread), falling back to the latest event.
  FeedItem deepLinkTarget(int? readAt) {
    if (readAt != null) {
      FeedItem? oldestUnread;
      for (final candidate in groupItems) {
        if (candidate.createdAt <= readAt) continue;
        if (oldestUnread == null ||
            candidate.createdAt < oldestUnread.createdAt) {
          oldestUnread = candidate;
        }
      }
      if (oldestUnread != null) return oldestUnread;
    }
    return item;
  }
}

/// Contextual row label — desktop's `getInboxTypeLabel`:
/// "DM from X" / "Mentioned in" / "Needs action in" / "Thread in" / "... in".
({String text, String? channelLabel}) inboxTypeLabel(
  InboxItem item, {
  required String? channelName,
  required bool isDm,
  required String senderLabel,
}) {
  if (isDm) {
    return (
      text: senderLabel.isNotEmpty ? 'DM from $senderLabel' : 'DM',
      channelLabel: null,
    );
  }

  final category = item.categories.firstOrNull ?? item.item.category;
  if (category == 'mention') {
    return (
      text: channelName != null ? 'Mentioned in' : 'Mentioned',
      channelLabel: channelName,
    );
  }
  if (category == 'needs_action') {
    return (
      text: channelName != null ? 'Needs action in' : 'Needs action',
      channelLabel: channelName,
    );
  }
  if (item.threadRootId != null) {
    return (
      text: channelName != null ? 'Thread in' : 'Thread',
      channelLabel: channelName,
    );
  }

  final headline = item.item.headline;
  return (
    text: channelName != null ? '$headline in' : headline,
    channelLabel: channelName,
  );
}

/// Whether [item] matches [filter] — desktop's `matchesInboxFilter`.
/// `reminders` and `drafts` are separate surfaces and never match here.
bool matchesInboxFilter(InboxItem item, InboxFilter filter) {
  return switch (filter) {
    InboxFilter.all => true,
    InboxFilter.thread => [
      item.item,
      ...item.groupItems,
    ].any((i) => isThreadReply(i.tags)),
    InboxFilter.conversations =>
      [item.item, ...item.groupItems].any((i) => isThreadReply(i.tags)) ||
          [item.item, ...item.groupItems].every(isChannelMainItem),
    InboxFilter.mention => item.categories.contains('mention'),
    InboxFilter.needsAction => item.categories.contains('needs_action'),
    InboxFilter.activity => item.categories.contains('activity'),
    InboxFilter.agentActivity => item.categories.contains('agent_activity'),
    InboxFilter.reminders || InboxFilter.drafts => false,
  };
}

const _channelMessageKinds = {9, 40002, 45001};

/// A top-level channel or DM message (not a thread reply, job, or approval).
bool isChannelMainItem(FeedItem item) =>
    item.channelId != null &&
    _channelMessageKinds.contains(item.kind) &&
    !isThreadReply(item.tags);

/// The Channels + Threads view treats each channel's main timeline like one
/// thread: top-level rows from the same channel collapse into a single row
/// keyed `channel:<id>`, ordered by latest activity alongside thread rows.
/// DM rows are already grouped per channel and pass through unchanged.
/// Mirrors desktop's `collapseChannelMainRows`.
List<InboxItem> collapseChannelMainRows(Iterable<InboxItem> items) {
  final rows = <InboxItem>[];
  final channelRowIndex = <String, int>{};

  for (final item in items) {
    final channelId = item.item.channelId;
    final isMainRow =
        channelId != null &&
        !item.conversationId.startsWith('dm:') &&
        [item.item, ...item.groupItems].every(isChannelMainItem);
    if (!isMainRow) {
      rows.add(item);
      continue;
    }

    final index = channelRowIndex[channelId];
    if (index == null) {
      channelRowIndex[channelId] = rows.length;
      rows.add(_withConversationId(item, 'channel:$channelId'));
      continue;
    }

    final existing = rows[index];
    final latest = item.latestActivityAt > existing.latestActivityAt
        ? item
        : existing;
    final categories = {...existing.categories, ...item.categories}.toList()
      ..sort((a, b) => categoryPriority(a).compareTo(categoryPriority(b)));
    rows[index] = InboxItem(
      conversationId: existing.conversationId,
      id: latest.id,
      item: latest.item,
      groupItems: List.unmodifiable([
        ...existing.groupItems,
        ...item.groupItems,
      ]),
      categories: List.unmodifiable(categories),
      isActionRequired: existing.isActionRequired || item.isActionRequired,
      latestActivityAt: latest.latestActivityAt,
    );
  }

  rows.sort((a, b) => b.latestActivityAt.compareTo(a.latestActivityAt));
  return rows;
}

InboxItem _withConversationId(InboxItem item, String conversationId) =>
    InboxItem(
      conversationId: conversationId,
      id: item.id,
      item: item.item,
      groupItems: item.groupItems,
      categories: item.categories,
      isActionRequired: item.isActionRequired,
      latestActivityAt: item.latestActivityAt,
    );

/// Group raw feed items into conversation rows sorted by latest activity.
/// [isDmChannel] identifies DM channels so their ordinary top-level messages
/// collapse into one row per DM conversation. Mirrors desktop's
/// `buildInboxItems`.
List<InboxItem> buildInboxItems(
  Iterable<FeedItem> feedItems, {
  bool Function(String channelId)? isDmChannel,
}) {
  final groups = <String, List<FeedItem>>{};
  for (final item in feedItems) {
    final channelId = item.channelId;
    final dmChannelId =
        channelId != null && (isDmChannel?.call(channelId) ?? false)
        ? channelId
        : null;
    final key = inboxConversationId(
      item.tags,
      item.id,
      dmChannelId: dmChannelId,
    );
    groups.putIfAbsent(key, () => []).add(item);
  }

  final rows = <InboxItem>[];
  for (final entry in groups.entries) {
    final items = entry.value;
    var latest = items.first;
    var latestActivityAt = 0;
    for (final item in items) {
      if (item.createdAt > latest.createdAt) latest = item;
      if (item.createdAt > latestActivityAt) latestActivityAt = item.createdAt;
    }
    final categories = items.map((i) => i.category).toSet().toList()
      ..sort((a, b) => categoryPriority(a).compareTo(categoryPriority(b)));

    rows.add(
      InboxItem(
        conversationId: entry.key,
        id: latest.id,
        item: latest,
        groupItems: List.unmodifiable(items),
        categories: List.unmodifiable(categories),
        isActionRequired: categories.contains('needs_action'),
        latestActivityAt: latestActivityAt,
      ),
    );
  }

  rows.sort((a, b) => b.latestActivityAt.compareTo(a.latestActivityAt));
  return rows;
}

/// Date-bucket label for section headers — desktop's `groupInboxItems`:
/// Today / Yesterday / weekday (within 7 days) / short date.
String inboxDayLabel(int unixSeconds, {DateTime? now}) {
  final current = now ?? DateTime.now();
  final date = DateTime.fromMillisecondsSinceEpoch(unixSeconds * 1000);
  final today = DateTime(current.year, current.month, current.day);
  final day = DateTime(date.year, date.month, date.day);
  final dayDiff = today.difference(day).inDays;

  if (dayDiff == 0) return 'Today';
  if (dayDiff == 1) return 'Yesterday';
  if (dayDiff < 7 && dayDiff > 0) {
    const weekdays = [
      'Monday',
      'Tuesday',
      'Wednesday',
      'Thursday',
      'Friday',
      'Saturday',
      'Sunday',
    ];
    return weekdays[date.weekday - 1];
  }
  const months = [
    'Jan',
    'Feb',
    'Mar',
    'Apr',
    'May',
    'Jun',
    'Jul',
    'Aug',
    'Sep',
    'Oct',
    'Nov',
    'Dec',
  ];
  final label = '${months[date.month - 1]} ${date.day}';
  return date.year == current.year ? label : '$label, ${date.year}';
}
