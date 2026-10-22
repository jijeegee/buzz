import 'dart:math' as math;

import 'package:flutter/foundation.dart';

import '../../shared/read_state/read_state_format.dart';
import '../../shared/relay/relay.dart';
import '../activity/inbox_item.dart';

/// Rows per relay `chat_list` page.
const chatListPageSize = 100;

/// Rows loaded in the background after the first page, as desktop does.
const chatListBackgroundRows = 400;

/// The relay caps unread and mention counts here and flags the overflow.
const chatUnreadCap = 99;

/// Threads shown under their channel in By channel before "+ N more".
const byChannelVisibleThreads = 3;

/// Threads each "+ 10 more" tap reveals under a channel.
const byChannelThreadStep = 10;

/// Message kinds the relay counts (kuzz-db `MESSAGE_KINDS`).
const _messageKinds = {9, 40002, 45001, 45003};

/// Close / reopen control replies: they change which threads are listed.
const _threadMarkerTags = {'kuzz:thread-closed', 'kuzz:thread-reopened'};

/// The relay answered without the `chat_list` envelope, or with
/// `chat_state_disabled`: it keeps no chat state. Callers fall back to the
/// channel list.
class ChatListUnsupportedException implements Exception {
  const ChatListUnsupportedException();

  @override
  String toString() => 'relay does not serve chat_list';
}

/// Keyset cursor returned by the relay as `next`.
@immutable
class ChatListCursor {
  const ChatListCursor({required this.atUs, required this.key});

  final int atUs;
  final String key;

  Map<String, dynamic> toJson() => {'at_us': atUs, 'key': key};

  static ChatListCursor? fromJson(Object? json) =>
      json is Map<String, dynamic> &&
          json['at_us'] is int &&
          json['key'] is String
      ? ChatListCursor(atUs: json['at_us'] as int, key: json['key'] as String)
      : null;

  @override
  bool operator ==(Object other) =>
      other is ChatListCursor && other.atUs == atUs && other.key == key;

  @override
  int get hashCode => Object.hash(atUs, key);
}

/// One relay `chat_list` row: a room main timeline or one live thread.
/// Mirrors desktop's `ChatListRow`.
@immutable
class ChatListRow {
  const ChatListRow({
    required this.isThread,
    required this.key,
    required this.at,
    required this.atUs,
    required this.channelId,
    required this.channelType,
    required this.channelName,
    this.archived = false,
    this.dmMembers = const [],
    this.groupChat = false,
    this.memberCount,
    this.rootId,
    this.threadName,
    this.closed = false,
    this.unread = 0,
    this.unreadOverflow = false,
    this.unreadMentions = 0,
    this.unreadMentionsOverflow = false,
    this.unreadMark = false,
    this.latest,
    this.root,
  });

  final bool isThread;

  /// Channel UUID for rooms, thread root id for threads.
  final String key;
  final int at;
  final int atUs;
  final String channelId;
  final String channelType;
  final String channelName;
  final bool archived;

  /// A DM's people, or up to 16 other active members of a group chat.
  final List<String> dmMembers;

  /// An ownerless chat of people (relay `group_chat`).
  final bool groupChat;

  /// A group chat's active members (relay `member_count`).
  final int? memberCount;
  final String? rootId;
  final String? threadName;
  final bool closed;
  final int unread;
  final bool unreadOverflow;
  final int unreadMentions;
  final bool unreadMentionsOverflow;
  final bool unreadMark;
  final NostrEvent? latest;
  final NostrEvent? root;

  bool get isDm => channelType == 'dm';

  /// A group chat: titled and drawn from its members, like a DM.
  bool get isGroupChat => groupChat && !isDm;

  /// A 1:1 or a group chat: a room of people rather than a managed group.
  bool get isPersonChat => isDm || isGroupChat;

  static ChatListRow? fromJson(Object? json) {
    if (json is! Map<String, dynamic>) return null;
    final type = json['type'];
    final key = json['key'];
    final at = json['at'];
    final atUs = json['at_us'];
    final channelId = json['channel_id'];
    if ((type != 'room' && type != 'thread') ||
        key is! String ||
        at is! int ||
        atUs is! int ||
        channelId is! String) {
      return null;
    }
    int count(String name) => json[name] is int ? json[name] as int : 0;
    bool flag(String name) => json[name] == true;
    final members = json['dm_members'];
    return ChatListRow(
      isThread: type == 'thread',
      key: key,
      at: at,
      atUs: atUs,
      channelId: channelId,
      channelType: json['channel_type'] is String
          ? json['channel_type'] as String
          : 'stream',
      channelName: json['channel_name'] is String
          ? json['channel_name'] as String
          : '',
      archived: flag('archived'),
      dmMembers: members is List
          ? [for (final member in members) ?(member is String ? member : null)]
          : const [],
      groupChat: flag('group_chat'),
      memberCount: json['member_count'] is int
          ? json['member_count'] as int
          : null,
      rootId: json['root_id'] is String ? json['root_id'] as String : null,
      threadName: json['thread_name'] is String
          ? json['thread_name'] as String
          : null,
      closed: flag('closed'),
      unread: count('unread'),
      unreadOverflow: flag('unread_overflow'),
      unreadMentions: count('unread_mentions'),
      unreadMentionsOverflow: flag('unread_mentions_overflow'),
      unreadMark: flag('unread_mark'),
      latest: _eventFromJson(json['latest']),
      root: _eventFromJson(json['root']),
    );
  }

  Map<String, dynamic> toJson() => {
    'type': isThread ? 'thread' : 'room',
    'key': key,
    'at': at,
    'at_us': atUs,
    'channel_id': channelId,
    'channel_type': channelType,
    'channel_name': channelName,
    'archived': archived,
    'dm_members': dmMembers,
    'group_chat': groupChat,
    'member_count': memberCount,
    'root_id': rootId,
    'thread_name': threadName,
    'closed': closed,
    'unread': unread,
    'unread_overflow': unreadOverflow,
    'unread_mentions': unreadMentions,
    'unread_mentions_overflow': unreadMentionsOverflow,
    'unread_mark': unreadMark,
    'latest': latest?.toJson(),
    'root': root?.toJson(),
  };

  ChatListRow copyWith({
    int? at,
    int? atUs,
    int? unread,
    bool? unreadOverflow,
    int? unreadMentions,
    bool? unreadMentionsOverflow,
    bool? unreadMark,
    NostrEvent? latest,
  }) => ChatListRow(
    isThread: isThread,
    key: key,
    at: at ?? this.at,
    atUs: atUs ?? this.atUs,
    channelId: channelId,
    channelType: channelType,
    channelName: channelName,
    archived: archived,
    dmMembers: dmMembers,
    groupChat: groupChat,
    memberCount: memberCount,
    rootId: rootId,
    threadName: threadName,
    closed: closed,
    unread: unread ?? this.unread,
    unreadOverflow: unreadOverflow ?? this.unreadOverflow,
    unreadMentions: unreadMentions ?? this.unreadMentions,
    unreadMentionsOverflow:
        unreadMentionsOverflow ?? this.unreadMentionsOverflow,
    unreadMark: unreadMark ?? this.unreadMark,
    latest: latest ?? this.latest,
    root: root,
  );
}

NostrEvent? _eventFromJson(Object? json) {
  if (json is! Map<String, dynamic>) return null;
  try {
    return NostrEvent.fromJson(json);
  } catch (_) {
    return null;
  }
}

@immutable
class ChatListPage {
  const ChatListPage({required this.rows, required this.next});

  final List<ChatListRow> rows;
  final ChatListCursor? next;
}

/// The loaded chats list. [fromCache] is set while the rows come from this
/// device's saved copy and the relay has not answered yet.
@immutable
class ChatListState {
  const ChatListState({
    required this.rows,
    required this.next,
    this.fromCache = false,
  });

  final List<ChatListRow> rows;
  final ChatListCursor? next;
  final bool fromCache;

  ChatListState copyWith({
    List<ChatListRow>? rows,
    ChatListCursor? next,
    bool clearNext = false,
    bool? fromCache,
  }) => ChatListState(
    rows: rows ?? this.rows,
    next: clearNext ? null : (next ?? this.next),
    fromCache: fromCache ?? this.fromCache,
  );
}

Map<String, dynamic> buildChatListFilter({
  required int limit,
  ChatListCursor? before,
  List<String> pinnedThreads = const [],
}) => {
  'chat_list': 1,
  'limit': limit.clamp(1, 100),
  if (before != null) 'before': before.toJson(),
  if (pinnedThreads.isNotEmpty) 'pinned_threads': pinnedThreads,
};

/// Parses a `chat_list` response; anything but the version-1 envelope
/// throws [ChatListUnsupportedException]. Malformed rows are skipped.
ChatListPage parseChatListResponse(Object? json) {
  if (json is! Map<String, dynamic> ||
      json['chat_list'] != 1 ||
      json['rows'] is! List) {
    throw const ChatListUnsupportedException();
  }
  return ChatListPage(
    rows: [for (final row in json['rows'] as List) ?ChatListRow.fromJson(row)],
    next: ChatListCursor.fromJson(json['next']),
  );
}

/// True for a relay error that means the relay keeps no chat state.
bool isChatStateDisabledError(Object error) =>
    error is ChatListUnsupportedException ||
    (error is RelayException &&
        error.statusCode == 404 &&
        error.toString().contains('chat_state_disabled'));

int _rowSortDesc(ChatListRow left, ChatListRow right) {
  final byTime = right.atUs.compareTo(left.atUs);
  return byTime != 0 ? byTime : right.key.compareTo(left.key);
}

/// Append a later page, keeping the first copy of any row.
ChatListState mergeChatListPage(ChatListState current, ChatListPage page) {
  final keys = {for (final row in current.rows) row.key};
  return ChatListState(
    rows: [
      ...current.rows,
      for (final row in page.rows)
        if (!keys.contains(row.key)) row,
    ],
    next: page.next,
  );
}

/// A refetched first page over the loaded list: rows past the new first page
/// stay (with their cursor), so a refetch reloads one page, not all of them.
ChatListState mergeRefetchedFirstPage(
  ChatListState? current,
  ChatListPage page,
) {
  final last = page.rows.lastOrNull;
  // A saved copy may be stale anywhere; only a relay-loaded list keeps rows.
  if (current == null ||
      current.fromCache ||
      page.next == null ||
      last == null) {
    return ChatListState(rows: page.rows, next: page.next);
  }
  final keys = {for (final row in page.rows) row.key};
  final older = [
    for (final row in current.rows)
      if (!keys.contains(row.key) && _rowSortDesc(last, row) < 0) row,
  ];
  if (older.isEmpty) return ChatListState(rows: page.rows, next: page.next);
  return ChatListState(rows: [...page.rows, ...older], next: current.next);
}

bool _hasThreadMarker(NostrEvent event, String rootId) => event.tags.any(
  (tag) =>
      tag.length >= 2 &&
      _threadMarkerTags.contains(tag[0]) &&
      tag[1].toLowerCase() == rootId.toLowerCase(),
);

({int count, bool overflow}) _bump(int count, bool overflow) =>
    count >= chatUnreadCap
    ? (count: chatUnreadCap, overflow: true)
    : (count: count + 1, overflow: overflow);

/// Fold one live message into the loaded list, as the relay's write path
/// does: a top-level (or broadcast) message moves its room row, a reply
/// moves its thread row; others' messages add unread (and mentions), your
/// own marks the row read. Rows only the relay can place (a new room, a
/// thread you just joined, a close or reopen) ask for a refetch. Mirrors
/// desktop's `applyLiveEventToChatList`.
///
/// [replay] is for events buffered during a fetch: one no newer than the
/// row's latest is already counted. [readAtOf] gives this device's read
/// position for a row; a row read past its latest message counts from zero.
({ChatListState state, bool refetch}) applyLiveEventToChatList(
  ChatListState current,
  NostrEvent event, {
  required String channelId,
  required String? currentPubkey,
  bool replay = false,
  int? Function(ChatListRow row)? readAtOf,
}) {
  if (!_messageKinds.contains(event.kind)) {
    return (state: current, refetch: false);
  }
  final rootId = threadReferenceOf(event.tags).rootId;
  if (rootId != null && _hasThreadMarker(event, rootId)) {
    return (state: current, refetch: true);
  }
  final me = currentPubkey?.toLowerCase() ?? '';
  final fromMe = me.isNotEmpty && event.pubkey.toLowerCase() == me;
  final mentionsMe =
      me.isNotEmpty && !fromMe && mentionedPubkeysOf(event.tags).contains(me);
  final targets = <String?>[
    ?rootId,
    if (rootId == null || isBroadcastReply(event.tags)) null,
  ];

  var rows = current.rows;
  var refetch = false;
  for (final target in targets) {
    final index = rows.indexWhere(
      (row) => target != null
          ? row.isThread && row.rootId == target
          : !row.isThread && row.channelId == channelId,
    );
    if (index == -1) {
      // An unknown room is new or beyond the loaded pages; an unknown thread
      // is listed only once you take part in it.
      refetch = refetch || target == null || fromMe || mentionsMe;
      continue;
    }
    final existing = rows[index];
    final latest = existing.latest;
    if (latest?.id == event.id) continue;
    if (latest != null &&
        (replay
            ? latest.createdAt >= event.createdAt
            : latest.createdAt > event.createdAt)) {
      continue;
    }
    final readAt = readAtOf?.call(existing);
    final readHere =
        readAt != null &&
        latest != null &&
        readAt >= latest.createdAt &&
        !existing.unreadMark;
    final base = readHere
        ? existing.copyWith(
            unread: 0,
            unreadOverflow: false,
            unreadMentions: 0,
            unreadMentionsOverflow: false,
          )
        : existing;
    final unread = fromMe
        ? (count: 0, overflow: false)
        : _bump(base.unread, base.unreadOverflow);
    final mentions = fromMe
        ? (count: 0, overflow: false)
        : mentionsMe
        ? _bump(base.unreadMentions, base.unreadMentionsOverflow)
        : (count: base.unreadMentions, overflow: base.unreadMentionsOverflow);
    final next = existing.copyWith(
      at: existing.at > event.createdAt ? existing.at : event.createdAt,
      atUs: existing.atUs > event.createdAt * 1000000
          ? existing.atUs
          : event.createdAt * 1000000,
      latest: event,
      unread: unread.count,
      unreadOverflow: unread.overflow,
      unreadMentions: mentions.count,
      unreadMentionsOverflow: mentions.overflow,
      unreadMark: fromMe ? false : existing.unreadMark,
    );
    rows = [for (var i = 0; i < rows.length; i++) i == index ? next : rows[i]];
  }
  if (identical(rows, current.rows)) return (state: current, refetch: refetch);
  return (
    state: current.copyWith(rows: [...rows]..sort(_rowSortDesc)),
    refetch: refetch,
  );
}

/// Pubkeys a message mentions (`p` tags).
Set<String> mentionedPubkeysOf(List<List<String>> tags) => {
  for (final tag in tags)
    if (tag.length >= 2 && tag[0] == 'p') tag[1].toLowerCase(),
};

/// True when any of [ids] is a row's shown latest message or thread root.
bool chatListShowsAny(ChatListState state, Iterable<String> ids) {
  final wanted = ids.toSet();
  if (wanted.isEmpty) return false;
  return state.rows.any(
    (row) =>
        (row.latest != null && wanted.contains(row.latest!.id)) ||
        (row.rootId != null && wanted.contains(row.rootId)),
  );
}

/// The read-state context a row's read position lives in.
String chatRowContextId(ChatListRow row) => row.isThread && row.rootId != null
    ? threadContextKey(row.rootId!)
    : row.channelId;

/// A row's unread state as shown, after this device's own read position.
@immutable
class ChatRowUnread {
  const ChatRowUnread({
    required this.count,
    required this.overflow,
    required this.mentions,
    required this.mentionsOverflow,
    required this.marked,
  });

  static const none = ChatRowUnread(
    count: 0,
    overflow: false,
    mentions: 0,
    mentionsOverflow: false,
    marked: false,
  );

  final int count;
  final bool overflow;
  final int mentions;
  final bool mentionsOverflow;

  /// Marked unread with nothing to count.
  final bool marked;

  bool get isUnread => count > 0 || overflow || marked;
}

/// Server counts, unless this device has already read past the row's latest
/// message (the mark-read mirror has not reached the relay yet). A row the
/// user marked unread stays marked.
ChatRowUnread chatRowUnread(ChatListRow row, int? ownReadAt) {
  final latest = row.latest;
  if (ownReadAt != null &&
      latest != null &&
      ownReadAt >= latest.createdAt &&
      !row.unreadMark) {
    return ChatRowUnread.none;
  }
  return ChatRowUnread(
    count: row.unread,
    overflow: row.unreadOverflow,
    mentions: row.unreadMentions,
    mentionsOverflow: row.unreadMentionsOverflow,
    marked: row.unreadMark && row.unread == 0,
  );
}

/// "99+" past the relay's cap.
String formatChatCount(int count, bool overflow) =>
    overflow || count > chatUnreadCap ? '$chatUnreadCap+' : '$count';

/// Whether a row is pinned: a room by its star, a thread by its pin.
bool isPinnedChatRow(
  ChatListRow row,
  Set<String> pinnedRoomIds,
  Set<String> pinnedThreadIds,
) => row.isThread
    ? pinnedThreadIds.contains(row.rootId ?? row.key)
    : pinnedRoomIds.contains(row.channelId);

/// Pinned rows first (rooms by star, threads by pin), newest first; then
/// everything else by activity.
List<ChatListRow> orderChatRows(
  Iterable<ChatListRow> rows,
  Set<String> pinnedRoomIds, {
  Set<String> pinnedThreadIds = const {},
}) {
  bool isPinned(ChatListRow row) =>
      isPinnedChatRow(row, pinnedRoomIds, pinnedThreadIds);
  final sorted = [...rows]..sort(_rowSortDesc);
  return [...sorted.where(isPinned), ...sorted.where((row) => !isPinned(row))];
}

/// A thread row's title: its name, else the root's first line, trimmed.
String chatThreadTitle(ChatListRow row) {
  final name = row.threadName?.trim();
  if (name != null && name.isNotEmpty) return name;
  final firstLine = row.root?.content
      .split('\n')
      .map((line) => line.trim())
      .where((line) => line.isNotEmpty)
      .firstOrNull;
  if (firstLine == null) return 'Thread';
  return firstLine.length > 80 ? '${firstLine.substring(0, 80)}…' : firstLine;
}

/// Shown instead of a blocked person's message.
const blockedMessageLabel = 'Message from a blocked user';

/// Shown for a message with no text left after clean-up.
const noMessageTextLabel = 'No message text';

const _previewMaxLength = 160;

final _spoiler = RegExp(r'\|\|[^|]*(?:\|(?!\|)[^|]*)*\|\|');
final _markdownImage = RegExp(r'!\[[^\]]*\]\([^)]*\)');
final _markdownLink = RegExp(r'\[([^\]]+)\]\([^)]*\)');
final _bareUrl = RegExp(r'<?(?:https?|kuzz)://\S+>?');
final _markdownMarks = RegExp(r'[`*_~>#|]');
final _whitespace = RegExp(r'\s+');

/// A message as one plain line, as desktop's `summarizeMessageLinkContent`:
/// control characters and line breaks become spaces, spoilers, images and
/// addresses go, links keep their text, markdown marks go, and past 160
/// characters it is cut at a word with "…". Empty reads "No message text".
String summarizeMessageText(String content) {
  final plain = String.fromCharCodes([
    for (final rune in content.runes)
      rune <= 0x1f || (rune >= 0x7f && rune <= 0x9f) ? 0x20 : rune,
  ]);
  final normalized = plain
      .replaceAll(_spoiler, ' ')
      .replaceAll(_markdownImage, ' ')
      .replaceAllMapped(_markdownLink, (match) => match.group(1)!)
      .replaceAll(_bareUrl, ' ')
      .replaceAll(_markdownMarks, ' ')
      .replaceAll(_whitespace, ' ')
      .trim();
  if (normalized.isEmpty) return noMessageTextLabel;
  final characters = normalized.runes.toList();
  if (characters.length <= _previewMaxLength) return normalized;
  final clipped = String.fromCharCodes(characters.take(_previewMaxLength - 1));
  final lastSpace = clipped.lastIndexOf(' ');
  final snippet = lastSpace > 96 ? clipped.substring(0, lastSpace) : clipped;
  return '${snippet.trimRight()}…';
}

/// The latest message's text alone, as other messengers show it: no author
/// name. A message with only attachments reads "Photo"; one from someone I
/// blocked never shows its text. Mirrors desktop's `chatRowPreviewLine`.
String chatRowPreviewText(
  NostrEvent event, {
  bool Function(String pubkey)? isBlocked,
}) {
  if (isBlocked?.call(event.pubkey.toLowerCase()) ?? false) {
    return blockedMessageLabel;
  }
  final text = summarizeMessageText(event.content);
  if (text == noMessageTextLabel &&
      event.tags.any((tag) => tag.isNotEmpty && tag[0] == 'imeta')) {
    return 'Photo';
  }
  return text;
}

/// The newest message among [events] (one channel's) that a row could show
/// instead of its latest when that one is from someone I blocked: no newer
/// than [before], not from a blocked author, and in the row's place: the
/// thread under [rootId], or the room's main timeline when [rootId] is null.
NostrEvent? latestVisibleChatMessage(
  Iterable<NostrEvent> events, {
  required String? rootId,
  required int before,
  required bool Function(String pubkey) isBlocked,
}) {
  NostrEvent? best;
  for (final event in events) {
    if (!_messageKinds.contains(event.kind) || event.createdAt > before) {
      continue;
    }
    if (isBlocked(event.pubkey.toLowerCase())) continue;
    final eventRoot = threadReferenceOf(event.tags).rootId;
    final inPlace = rootId != null
        ? event.id == rootId || eventRoot == rootId
        : eventRoot == null || isBroadcastReply(event.tags);
    if (!inPlace) continue;
    if (best == null || event.createdAt > best.createdAt) best = event;
  }
  return best;
}

/// One entry of the By channel view.
abstract class ChatGroupEntry {
  const ChatGroupEntry();
}

/// A room row (DM, group DM or channel main timeline), or the header of a
/// channel whose room row is not loaded (only its threads are).
final class ChatGroupRoom extends ChatGroupEntry {
  const ChatGroupRoom({required this.channelId, required this.row});

  final String channelId;

  /// Null when only the channel's threads are listed.
  final ChatListRow? row;
}

/// A thread row under its channel.
final class ChatGroupThread extends ChatGroupEntry {
  const ChatGroupThread(this.row);

  final ChatListRow row;
}

/// "+ N more threads", "+ 10 more · N left" and "Show less" under a channel.
final class ChatGroupMore extends ChatGroupEntry {
  const ChatGroupMore({
    required this.channelId,
    required this.hidden,
    required this.hiddenUnread,
    required this.expanded,
  });

  final String channelId;

  /// Threads still hidden; 0 once every thread shows.
  final int hidden;

  /// Unread messages in the still hidden threads.
  final int hiddenUnread;

  /// Shows threads past the first ones, so "Show less" is offered.
  final bool expanded;
}

/// Groups ordered rows by channel: each channel once, at the position of its
/// newest row, followed by its threads newest first (pinned threads first and
/// always shown). [byChannelVisibleThreads] threads show, plus
/// [byChannelThreadStep] more per step in [expandedSteps] (channel id to
/// "+ 10 more" taps). [unreadOf] gives a row's shown unread count.
List<ChatGroupEntry> groupChatRowsByChannel(
  List<ChatListRow> ordered, {
  required Map<String, int> expandedSteps,
  required int Function(ChatListRow row) unreadOf,
  Set<String> pinnedThreadIds = const {},
}) {
  final rooms = <String, ChatListRow>{};
  final threads = <String, List<ChatListRow>>{};
  final order = <String>[];
  for (final row in ordered) {
    final channelId = row.channelId;
    if (!rooms.containsKey(channelId) && !threads.containsKey(channelId)) {
      order.add(channelId);
    }
    if (row.isThread) {
      (threads[channelId] ??= []).add(row);
    } else {
      rooms[channelId] ??= row;
    }
  }
  final entries = <ChatGroupEntry>[];
  for (final channelId in order) {
    entries.add(ChatGroupRoom(channelId: channelId, row: rooms[channelId]));
    final all = threads[channelId] ?? const <ChatListRow>[];
    bool pinned(ChatListRow row) =>
        pinnedThreadIds.contains(row.rootId ?? row.key);
    final list = [...all.where(pinned), ...all.where((row) => !pinned(row))];
    final limit = math.max(byChannelVisibleThreads, all.where(pinned).length);
    final shown = math.min(
      list.length,
      limit + (expandedSteps[channelId] ?? 0) * byChannelThreadStep,
    );
    entries.addAll(list.take(shown).map(ChatGroupThread.new));
    if (list.length > limit) {
      final hidden = list.skip(shown);
      entries.add(
        ChatGroupMore(
          channelId: channelId,
          hidden: list.length - shown,
          hiddenUnread: hidden.fold(0, (sum, row) => sum + unreadOf(row)),
          expanded: shown > limit,
        ),
      );
    }
  }
  return entries;
}
