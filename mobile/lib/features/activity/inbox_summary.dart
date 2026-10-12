import 'dart:math' as math;

import 'package:flutter/foundation.dart';

import '../../shared/read_state/read_state_format.dart';
import '../../shared/relay/relay.dart';
import 'feed_item.dart';
import 'inbox_item.dart';

/// Rows per relay `inbox_summary` page.
const inboxSummaryPageSize = 50;

/// Rows loaded in the background after the first page, as desktop does.
const inboxSummaryBackgroundRows = 300;

/// The relay accepts at most this many read markers per request.
const inboxSummaryMaxReadMarkers = 4096;

/// The relay caps unread counts here and flags anything above as overflow.
const inboxSummaryUnreadCap = 99;

/// Message kinds the chat list shows, as the per-room loading did.
const _summaryKinds = {9, 40002, 45001, 45003};

/// The relay answered `/query` without the summary envelope (an older relay
/// that ignores the opt-in keys). Callers fall back to per-room loading.
class InboxSummaryUnsupportedException implements Exception {
  const InboxSummaryUnsupportedException();

  @override
  String toString() => 'relay does not support inbox_summary';
}

/// Keyset cursor returned by the relay as `next`.
@immutable
class InboxSummaryCursor {
  const InboxSummaryCursor({required this.atUs, required this.key});

  final int atUs;
  final String key;

  Map<String, dynamic> toJson() => {'at_us': atUs, 'key': key};

  @override
  bool operator ==(Object other) =>
      other is InboxSummaryCursor && other.atUs == atUs && other.key == key;

  @override
  int get hashCode => Object.hash(atUs, key);
}

/// One relay `inbox_summary` row: a room main timeline or one thread.
/// Mirrors desktop's `InboxSummaryRow`.
@immutable
class InboxSummaryRow {
  const InboxSummaryRow({
    required this.isThread,
    required this.channelId,
    required this.channelType,
    required this.rootId,
    required this.at,
    required this.atUs,
    required this.key,
    required this.replyCount,
    required this.unread,
    required this.unreadOverflow,
    required this.latest,
    required this.root,
  });

  final bool isThread;
  final String channelId;
  final String channelType;
  final String? rootId;
  final int at;
  final int atUs;
  final String key;
  final int? replyCount;

  /// Unread messages from others after the sent marker; null when unknown.
  final int? unread;
  final bool unreadOverflow;
  final NostrEvent? latest;
  final NostrEvent? root;

  bool get isDm => channelType == 'dm';

  static InboxSummaryRow? fromJson(Object? json) {
    if (json is! Map<String, dynamic>) return null;
    final type = json['type'];
    final channelId = json['channel_id'];
    final at = json['at'];
    final atUs = json['at_us'];
    final key = json['key'];
    if ((type != 'room' && type != 'thread') ||
        channelId is! String ||
        at is! int ||
        atUs is! int ||
        key is! String) {
      return null;
    }
    final rootId = json['root_id'];
    if (type == 'thread' && rootId is! String) return null;
    return InboxSummaryRow(
      isThread: type == 'thread',
      channelId: channelId,
      channelType: json['channel_type'] as String? ?? 'stream',
      rootId: rootId is String ? rootId : null,
      at: at,
      atUs: atUs,
      key: key,
      replyCount: json['reply_count'] as int?,
      unread: json['unread'] as int?,
      unreadOverflow: json['unread_overflow'] == true,
      latest: _event(json['latest']),
      root: _event(json['root']),
    );
  }

  static NostrEvent? _event(Object? json) {
    if (json is! Map<String, dynamic>) return null;
    try {
      return NostrEvent.fromJson(json);
    } catch (_) {
      return null;
    }
  }

  InboxSummaryRow _withLiveEvent(NostrEvent event, {required bool fromOther}) {
    final unread = this.unread;
    final nextUnread = unread != null && fromOther ? unread + 1 : unread;
    return InboxSummaryRow(
      isThread: isThread,
      channelId: channelId,
      channelType: channelType,
      rootId: rootId,
      at: event.createdAt,
      atUs: math.max(atUs, event.createdAt * 1000000),
      key: key,
      replyCount: isThread ? (replyCount ?? 0) + 1 : null,
      unread: nextUnread == null
          ? null
          : math.min(nextUnread, inboxSummaryUnreadCap),
      unreadOverflow:
          unreadOverflow ||
          (nextUnread != null && nextUnread > inboxSummaryUnreadCap),
      latest: event,
      root: root,
    );
  }
}

@immutable
class InboxSummaryPage {
  const InboxSummaryPage({required this.rows, required this.next});

  final List<InboxSummaryRow> rows;
  final InboxSummaryCursor? next;
}

/// The loaded summary plus the read markers its counts were taken against.
@immutable
class InboxSummaryState {
  const InboxSummaryState({
    required this.rows,
    required this.next,
    required this.sentRead,
  });

  final List<InboxSummaryRow> rows;
  final InboxSummaryCursor? next;

  /// The read markers sent with the first page. Later pages count against
  /// the same markers.
  final Map<String, int> sentRead;

  /// False when read state had not loaded yet, so counts are unknown.
  bool get sentReadMarkers => sentRead.isNotEmpty;

  InboxSummaryState copyWith({
    List<InboxSummaryRow>? rows,
    InboxSummaryCursor? Function()? next,
  }) => InboxSummaryState(
    rows: rows ?? this.rows,
    next: next == null ? this.next : next(),
    sentRead: sentRead,
  );
}

/// The `/query` filter for one summary page. Mirrors desktop's
/// `build_inbox_summary_filter`.
Map<String, dynamic> buildInboxSummaryFilter({
  required int limit,
  required InboxSummaryCursor? before,
  required Map<String, int> read,
}) => {
  'inbox_summary': 1,
  'limit': limit.clamp(1, 100),
  'read': read,
  if (before != null) 'before': before.toJson(),
};

/// Own channel and `thread:<root>` markers for the summary's unread counts.
/// `msg:` contexts are left out; above the relay's cap the newest win.
Map<String, int> inboxSummaryReadContexts(Map<String, int> contexts) {
  final entries = [
    for (final entry in contexts.entries)
      if (!entry.key.startsWith(msgContextPrefix) && entry.value >= 0) entry,
  ];
  if (entries.length > inboxSummaryMaxReadMarkers) {
    entries.sort((a, b) => b.value.compareTo(a.value));
    entries.removeRange(inboxSummaryMaxReadMarkers, entries.length);
  }
  return {for (final entry in entries) entry.key: entry.value};
}

/// Parses a `/query` response. Anything other than a version-1 summary
/// envelope throws [InboxSummaryUnsupportedException]; malformed rows are
/// skipped.
InboxSummaryPage parseInboxSummaryResponse(Object? json) {
  if (json is! Map<String, dynamic> ||
      json['inbox_summary'] != 1 ||
      json['rows'] is! List) {
    throw const InboxSummaryUnsupportedException();
  }
  final next = json['next'];
  return InboxSummaryPage(
    rows: [
      for (final row in json['rows'] as List) ?InboxSummaryRow.fromJson(row),
    ],
    next:
        next is Map<String, dynamic> &&
            next['at_us'] is int &&
            next['key'] is String
        ? InboxSummaryCursor(
            atUs: next['at_us'] as int,
            key: next['key'] as String,
          )
        : null,
  );
}

/// Appends a later page, keeping the first copy of any row.
InboxSummaryState mergeInboxSummaryPage(
  InboxSummaryState current,
  InboxSummaryPage page,
) {
  final keys = {for (final row in current.rows) row.key};
  return current.copyWith(
    rows: [
      ...current.rows,
      for (final row in page.rows)
        if (keys.add(row.key)) row,
    ],
    next: () => page.next,
  );
}

int _compareRowsNewestFirst(InboxSummaryRow left, InboxSummaryRow right) {
  final byTime = right.atUs.compareTo(left.atUs);
  return byTime != 0 ? byTime : right.key.compareTo(left.key);
}

/// Folds one live event into the loaded summary: only the room or thread row
/// it belongs to changes and moves to the top. Returns [current] unchanged for
/// events that move no row (other kinds, duplicates, events older than the
/// row). Mirrors desktop's `applyLiveEventToInboxSummary`.
InboxSummaryState applyLiveEventToInboxSummary(
  InboxSummaryState current,
  NostrEvent event, {
  required String channelId,
  required String channelType,
  required String? currentPubkey,
}) {
  if (!_summaryKinds.contains(event.kind)) return current;
  final rootId = threadReferenceOf(event.tags).rootId;
  final fromOther =
      currentPubkey != null &&
      currentPubkey.isNotEmpty &&
      event.pubkey.toLowerCase() != currentPubkey.toLowerCase();
  final targets = <String?>[
    ?rootId,
    if (rootId == null || isBroadcastReply(event.tags)) null,
  ];

  var rows = current.rows;
  for (final targetRoot in targets) {
    final index = rows.indexWhere(
      (row) => targetRoot != null
          ? row.isThread && row.rootId == targetRoot
          : !row.isThread && row.channelId == channelId,
    );
    final existing = index == -1 ? null : rows[index];
    final latest = existing?.latest;
    if (latest?.id == event.id) continue;
    // Relay `at` is a receive time; compare message times only.
    if (latest != null && latest.createdAt > event.createdAt) continue;
    final base =
        existing ??
        InboxSummaryRow(
          isThread: targetRoot != null,
          channelId: channelId,
          channelType: channelType,
          rootId: targetRoot,
          at: event.createdAt,
          atUs: event.createdAt * 1000000,
          key: targetRoot ?? channelId,
          replyCount: targetRoot != null ? 0 : null,
          unread: null,
          unreadOverflow: false,
          latest: null,
          root: null,
        );
    final next = base._withLiveEvent(event, fromOther: fromOther);
    rows = index == -1
        ? [...rows, next]
        : [for (var i = 0; i < rows.length; i++) i == index ? next : rows[i]];
  }
  if (identical(rows, current.rows)) return current;
  return current.copyWith(rows: [...rows]..sort(_compareRowsNewestFirst));
}

/// True when any of [ids] is a row's shown latest message or thread root, so
/// a deletion of it must reload the summary.
bool inboxSummaryShowsAny(InboxSummaryState state, Iterable<String> ids) {
  final wanted = ids.toSet();
  if (wanted.isEmpty) return false;
  return state.rows.any(
    (row) =>
        (row.latest != null && wanted.contains(row.latest!.id)) ||
        (row.rootId != null && wanted.contains(row.rootId)),
  );
}

/// Feed items for the existing inbox pipeline: each row's latest message,
/// plus a channel thread's root so the thread groups under its root id.
List<FeedItem> inboxSummaryFeedItems(Iterable<InboxSummaryRow> rows) {
  final items = <FeedItem>[];
  for (final row in rows) {
    final latest = row.latest;
    if (latest == null || !_summaryKinds.contains(latest.kind)) continue;
    // A DM root would join the DM's own row instead of its thread.
    final root = row.isThread && !row.isDm ? row.root : null;
    for (final event in [?root, latest]) {
      if (_summaryKinds.contains(event.kind)) {
        items.add(_summaryFeedItem(event, row));
      }
    }
  }
  return items;
}

/// Adds the room rows of [summaryRows] that [rows] lost. A room's latest
/// message can also be the root of a replied-to thread; the inbox groups that
/// event under its thread, so the room's `channel:<id>` row would never be
/// built. Each missing room row is built from its latest message directly.
List<InboxItem> withInboxSummaryRoomRows(
  List<InboxItem> rows,
  Iterable<InboxSummaryRow> summaryRows,
) {
  final present = {for (final row in rows) row.conversationId};
  final added = <InboxItem>[];
  for (final row in summaryRows) {
    final latest = row.latest;
    if (row.isThread ||
        latest == null ||
        !_summaryKinds.contains(latest.kind)) {
      continue;
    }
    final conversationId = inboxSummaryConversationId(row);
    if (!present.add(conversationId)) continue;
    final item = _summaryFeedItem(latest, row);
    added.add(
      InboxItem(
        conversationId: conversationId,
        id: item.id,
        item: item,
        groupItems: List.unmodifiable([item]),
        categories: const ['activity'],
        isActionRequired: false,
        latestActivityAt: item.createdAt,
      ),
    );
  }
  if (added.isEmpty) return rows;
  return [...rows, ...added]
    ..sort((a, b) => b.latestActivityAt.compareTo(a.latestActivityAt));
}

FeedItem _summaryFeedItem(NostrEvent event, InboxSummaryRow row) => FeedItem(
  id: event.id,
  kind: event.kind,
  pubkey: event.pubkey,
  content: event.content,
  createdAt: event.createdAt,
  channelId: row.channelId,
  channelName: '',
  tags: event.tags,
  category: 'activity',
);

/// The Channels + Threads row id a summary row stands for on mobile: threads
/// group by root (DM threads included), rooms collapse to `channel:<id>` or
/// `dm:<id>`.
String inboxSummaryConversationId(InboxSummaryRow row) {
  if (row.isThread && row.rootId != null) return row.rootId!;
  return row.isDm ? 'dm:${row.channelId}' : 'channel:${row.channelId}';
}

/// A row's server unread count and the marker and message time it was taken
/// at. Mirrors desktop's `InboxServerUnread`.
@immutable
class InboxServerUnread {
  const InboxServerUnread({
    required this.count,
    required this.overflow,
    required this.latestAt,
    required this.sentReadAt,
  });

  final int count;
  final bool overflow;

  /// created_at of the row's latest message (not the relay receive time).
  final int latestAt;

  /// Effective read marker sent for this row.
  final int? sentReadAt;
}

/// The marker the relay counts a row against: the room marker, or for a
/// thread the later of its thread and room markers.
int? inboxSummaryRowReadAt({
  required String channelId,
  required String? threadRootId,
  required int? Function(String contextId) markerOf,
}) => maxReadAt([
  markerOf(channelId),
  if (threadRootId != null) markerOf(threadContextKey(threadRootId)),
]);

/// Server unread counts keyed by mobile conversation id.
Map<String, InboxServerUnread> inboxSummaryUnread(InboxSummaryState state) {
  final unread = <String, InboxServerUnread>{};
  for (final row in state.rows) {
    final count = row.unread;
    if (count == null) continue;
    unread[inboxSummaryConversationId(row)] = InboxServerUnread(
      count: count,
      overflow: row.unreadOverflow,
      latestAt: row.latest?.createdAt ?? row.at,
      sentReadAt: inboxSummaryRowReadAt(
        channelId: row.channelId,
        threadRootId: row.isThread ? row.rootId : null,
        markerOf: (contextId) => state.sentRead[contextId],
      ),
    );
  }
  return unread;
}

/// The relay's unread count to show on an unread chat-list row, or null when
/// the current read state is the truth: the row is read, or its marker moved
/// since the summary was counted (for example, the user just marked it read).
/// Mirrors desktop's `withServerUnreadCounts`.
InboxServerUnread? inboxRowServerUnread(
  InboxItem row,
  Map<String, InboxServerUnread> unread, {
  required bool isDone,
  required int? Function(String contextId) markerOf,
}) {
  final server = unread[row.conversationId];
  final channelId = row.item.channelId;
  if (server == null || isDone || channelId == null) return null;
  final marker = inboxSummaryRowReadAt(
    channelId: channelId,
    threadRootId:
        row.conversationId.startsWith('channel:') ||
            row.conversationId.startsWith('dm:')
        ? null
        : row.conversationId,
    markerOf: markerOf,
  );
  if (marker != server.sentReadAt) return null;
  if (marker != null && marker >= server.latestAt) return null;
  return server;
}
