import 'dart:convert';

import '../../shared/relay/relay.dart';
import 'channel_event_order.dart';

/// Replies a thread opens with: about one screen of conversation and more.
const threadWindowPageReplies = 50;

/// Kind of the relay-signed overlay that closes every thread window.
const threadWindowBoundsKind = 39007;

/// Conversation kinds a thread window pages (NIP-CW thread mode).
const threadWindowKinds = [EventKind.streamMessage, EventKind.streamMessageV2];

/// Kinds that count as reply rows when folding pages; everything else in the
/// cache (reactions, edits, deletions) rides along.
const _threadRowKinds = {
  EventKind.streamMessage,
  EventKind.streamMessageV2,
  45001,
  45003,
};

bool _isReplyRow(NostrEvent event) => _threadRowKinds.contains(event.kind);

/// Whether [event] is a reply row the window holds or rules out by span:
/// rows no newer than [state]'s `newestCovered` come from the window alone.
/// Other events (reactions, edits, deletions) and newer live rows do not.
bool isCoveredByThreadWindow(NostrEvent event, ThreadWindowState state) {
  final covered = state.newestCovered;
  return covered != null && _isReplyRow(event) && !_isNewerThan(event, covered);
}

/// The newest reply row among [events], as a window cursor.
ThreadWindowCursor? newestThreadRow(Iterable<NostrEvent> events) =>
    _newestOf(events.where(_isReplyRow));

/// A position in a thread: a reply's time and id.
class ThreadWindowCursor {
  final int createdAt;
  final String eventId;

  const ThreadWindowCursor({required this.createdAt, required this.eventId});

  factory ThreadWindowCursor.of(NostrEvent event) =>
      ThreadWindowCursor(createdAt: event.createdAt, eventId: event.id);

  @override
  bool operator ==(Object other) =>
      other is ThreadWindowCursor &&
      other.createdAt == createdAt &&
      other.eventId == eventId;

  @override
  int get hashCode => Object.hash(createdAt, eventId);
}

/// One newest-first page of a thread as the relay serves it.
class ThreadWindowPage {
  final List<NostrEvent> events;
  final bool hasMore;

  /// Cursor for the next older page, when [hasMore].
  final ThreadWindowCursor? nextCursor;

  const ThreadWindowPage({
    required this.events,
    required this.hasMore,
    this.nextCursor,
  });
}

/// How far a thread's cached replies reach. Replies load newest first:
/// [olderCursor] is where the next older page starts and [newestCovered] the
/// newest reply a relay read has vouched for, so a later newest page can tell
/// whether it connects to what is cached. Mirrors desktop's
/// `ThreadWindowState`.
class ThreadWindowState {
  final bool hasOlder;
  final ThreadWindowCursor? olderCursor;
  final ThreadWindowCursor? newestCovered;

  const ThreadWindowState({
    required this.hasOlder,
    this.olderCursor,
    this.newestCovered,
  });
}

/// Cached replies of one thread and how far they reach.
class ThreadWindow {
  final List<NostrEvent> events;
  final ThreadWindowState state;

  const ThreadWindow({required this.events, required this.state});
}

/// Relay order is newest first, then ascending id within a second.
bool _isOlderThan(NostrEvent event, ThreadWindowCursor cursor) =>
    event.createdAt < cursor.createdAt ||
    (event.createdAt == cursor.createdAt &&
        event.id.compareTo(cursor.eventId) > 0);

bool _isNewerThan(NostrEvent event, ThreadWindowCursor cursor) =>
    !_isOlderThan(event, cursor) && event.id != cursor.eventId;

ThreadWindowCursor? _oldestOf(Iterable<NostrEvent> events) {
  NostrEvent? oldest;
  for (final event in events) {
    if (oldest == null || _isOlderThan(event, ThreadWindowCursor.of(oldest))) {
      oldest = event;
    }
  }
  return oldest == null ? null : ThreadWindowCursor.of(oldest);
}

ThreadWindowCursor? _newestOf(Iterable<NostrEvent> events) {
  NostrEvent? newest;
  for (final event in events) {
    if (newest == null || _isNewerThan(event, ThreadWindowCursor.of(newest))) {
      newest = event;
    }
  }
  return newest == null ? null : ThreadWindowCursor.of(newest);
}

/// True when [cursor] sits at or before [bound] in the thread.
bool _reachesBack(ThreadWindowCursor cursor, ThreadWindowCursor bound) =>
    cursor.createdAt < bound.createdAt ||
    (cursor.createdAt == bound.createdAt &&
        cursor.eventId.compareTo(bound.eventId) >= 0);

List<NostrEvent> _sorted(Iterable<NostrEvent> events) {
  final byId = <String, NostrEvent>{};
  for (final event in events) {
    byId[event.id] = event;
  }
  return byId.values.toList()..sort(compareThreadRepliesChronologically);
}

({bool hasOlder, ThreadWindowCursor? olderCursor}) _olderState(
  ThreadWindowPage page,
) => page.hasMore && page.nextCursor != null
    ? (hasOlder: true, olderCursor: page.nextCursor)
    : (hasOlder: false, olderCursor: null);

/// Folds a freshly fetched newest page into the cached replies.
///
/// The page is authoritative for the rows it covers, so cached rows inside
/// its span but missing from it (deleted since) drop out. Older cached rows
/// are kept only when the page reaches back to the newest row an earlier
/// relay read covered; otherwise replies may have been missed in between, so
/// the cache restarts from the page. Rows newer than the page and non-row
/// events are kept. Mirrors desktop's `mergeNewestThreadPage`.
ThreadWindow mergeNewestThreadPage(
  ThreadWindow? cached,
  ThreadWindowPage page,
) {
  final pageIds = {for (final event in page.events) event.id};
  final pageRows = page.events.where(_isReplyRow);
  final pageOldest = _oldestOf(pageRows);
  final pageNewest = _newestOf(pageRows);
  final previous = (cached?.events ?? const <NostrEvent>[])
      .where((event) => !pageIds.contains(event.id))
      .toList();
  final previousRows = previous.where(_isReplyRow).toList();
  final carried = previous.where((event) => !_isReplyRow(event));
  final newerRows = pageNewest == null
      ? previousRows
      : previousRows.where((event) => _isNewerThan(event, pageNewest));
  final keep = [...page.events, ...newerRows, ...carried];
  if (pageOldest == null || !page.hasMore) {
    // The page holds the whole thread.
    return ThreadWindow(
      events: _sorted(keep),
      state: ThreadWindowState(hasOlder: false, newestCovered: pageNewest),
    );
  }
  final covered = cached?.state.newestCovered;
  if (covered == null || !_reachesBack(pageOldest, covered)) {
    final older = _olderState(page);
    return ThreadWindow(
      events: _sorted(keep),
      state: ThreadWindowState(
        hasOlder: older.hasOlder,
        olderCursor: older.olderCursor,
        newestCovered: pageNewest,
      ),
    );
  }
  final olderRows = previousRows.where(
    (event) => _isOlderThan(event, pageOldest),
  );
  return ThreadWindow(
    events: _sorted([...olderRows, ...keep]),
    state: ThreadWindowState(
      hasOlder: cached!.state.hasOlder,
      olderCursor: cached.state.olderCursor,
      newestCovered: pageNewest,
    ),
  );
}

/// Folds an older page (fetched from `olderCursor`) into the cached replies.
/// Mirrors desktop's `mergeOlderThreadPage`.
ThreadWindow mergeOlderThreadPage(ThreadWindow? cached, ThreadWindowPage page) {
  final events = _sorted([
    ...page.events,
    ...(cached?.events ?? const <NostrEvent>[]),
  ]);
  final older = _olderState(page);
  return ThreadWindow(
    events: events,
    state: ThreadWindowState(
      hasOlder: older.hasOlder,
      olderCursor: older.olderCursor,
      newestCovered:
          cached?.state.newestCovered ?? _newestOf(events.where(_isReplyRow)),
    ),
  );
}

/// The newest-first reply window of one thread, starting at [cursor] (the
/// oldest reply already loaded) or at the newest reply when null.
NostrFilter threadWindowFilter({
  required String channelId,
  required String rootId,
  ThreadWindowCursor? cursor,
  int limit = threadWindowPageReplies,
}) => NostrFilter(
  kinds: threadWindowKinds,
  tags: {
    '#h': [channelId],
    '#e': [rootId],
  },
  limit: limit,
  until: cursor?.createdAt,
  // Rows only: reactions and edits reach thread rows through their own paths.
  extensions: {
    'thread_window': true,
    'include_aux': false,
    if (cursor != null) 'before_id': cursor.eventId,
  },
);

/// Reads one thread window out of a `/query` response: the events up to the
/// `39007` bounds overlay tagged with this thread. Returns null when the relay
/// served no window for it (no access, or a relay without thread windows).
ThreadWindowPage? parseThreadWindowPage(
  List<NostrEvent> events, {
  required String channelId,
  required String rootId,
}) {
  final segment = <NostrEvent>[];
  for (final event in events) {
    if (event.kind != threadWindowBoundsKind) {
      segment.add(event);
      continue;
    }
    if (event.getTagValue('h') != channelId ||
        event.getTagValue('e') != rootId) {
      segment.clear();
      continue;
    }
    Object? bounds;
    try {
      bounds = jsonDecode(event.content);
    } on FormatException {
      bounds = null;
    }
    final map = bounds is Map ? bounds : const {};
    final hasMore = map['has_more'] == true;
    final next = map['next_cursor'];
    final nextCursor =
        next is Map && next['created_at'] is int && next['id'] is String
        ? ThreadWindowCursor(
            createdAt: next['created_at'] as int,
            eventId: next['id'] as String,
          )
        : null;
    return ThreadWindowPage(
      events: List.unmodifiable(segment),
      hasMore: hasMore && nextCursor != null,
      nextCursor: hasMore ? nextCursor : null,
    );
  }
  return null;
}
