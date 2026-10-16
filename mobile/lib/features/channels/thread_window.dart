import 'dart:convert';

import '../../shared/relay/relay.dart';
import 'channel_event_order.dart';
import 'window_anchor.dart';

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
/// Other events (reactions, edits, deletions) and newer live rows do not,
/// except while newer pages remain: a live row then sits past rows not yet
/// loaded, so it waits for those pages instead of showing past the gap.
bool isCoveredByThreadWindow(NostrEvent event, ThreadWindowState state) {
  if (!_isReplyRow(event)) return false;
  if (state.hasNewer) return true;
  final covered = state.newestCovered;
  return covered != null && !_isNewerThan(event, covered);
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

/// One page of a thread as the relay serves it: newest first for an older
/// page, oldest first for a newer one.
class ThreadWindowPage {
  final List<NostrEvent> events;
  final bool hasMore;

  /// Cursor for the next page in [direction], when [hasMore].
  final ThreadWindowCursor? nextCursor;
  final WindowPageDirection direction;

  const ThreadWindowPage({
    required this.events,
    required this.hasMore,
    this.nextCursor,
    this.direction = WindowPageDirection.older,
  });
}

/// How far a thread's cached replies reach. Replies load newest first:
/// [olderCursor] is where the next older page starts and [newestCovered] the
/// newest reply a relay read has vouched for, so a later newest page can tell
/// whether it connects to what is cached. Mirrors desktop's
/// `ThreadWindowState`.
///
/// A window opened at a reply also pages forward: while [hasNewer], replies
/// after the loaded ones are still to be fetched from [newerCursor].
class ThreadWindowState {
  final bool hasOlder;
  final ThreadWindowCursor? olderCursor;
  final ThreadWindowCursor? newestCovered;
  final bool hasNewer;
  final ThreadWindowCursor? newerCursor;

  const ThreadWindowState({
    required this.hasOlder,
    this.olderCursor,
    this.newestCovered,
    this.hasNewer = false,
    this.newerCursor,
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
    // A window opened at a reply has not paged up to this newest page yet:
    // restarting from the page would drop what the reader is looking at.
    // Keep it; its newer pages close the gap.
    if (cached != null && cached.state.hasNewer) return cached;
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
      hasNewer: cached?.state.hasNewer ?? false,
      newerCursor: cached?.state.newerCursor,
    ),
  );
}

/// Opens the window at an anchor: [older] runs back from the anchor cursor
/// (the anchor reply first, when there is one) and [newer] forward from it.
/// Cached rows are replaced, since they may not border these pages; other
/// cached events ride along.
ThreadWindow mergeMiddleThreadPages(
  ThreadWindow? cached,
  ThreadWindowPage older,
  ThreadWindowPage newer,
) {
  if (older.direction != WindowPageDirection.older ||
      newer.direction != WindowPageDirection.newer) {
    throw ArgumentError('A middle thread window needs older and newer pages.');
  }
  final carried = (cached?.events ?? const <NostrEvent>[]).where(
    (event) => !_isReplyRow(event),
  );
  final events = _sorted([...older.events, ...newer.events, ...carried]);
  final olderState = _olderState(older);
  final newerState = _newerState(newer);
  return ThreadWindow(
    events: events,
    state: ThreadWindowState(
      hasOlder: olderState.hasOlder,
      olderCursor: olderState.olderCursor,
      newestCovered: _newestOf(events.where(_isReplyRow)),
      hasNewer: newerState.hasNewer,
      newerCursor: newerState.newerCursor,
    ),
  );
}

/// Starts the window over at a newest [page], dropping every cached row (a
/// window opened at a reply would otherwise be kept); other cached events
/// ride along.
ThreadWindow restartThreadWindowAtNewest(
  ThreadWindow? cached,
  ThreadWindowPage page,
) => mergeNewestThreadPage(
  ThreadWindow(
    events: [
      for (final event in cached?.events ?? const <NostrEvent>[])
        if (!_isReplyRow(event)) event,
    ],
    state: const ThreadWindowState(hasOlder: false),
  ),
  page,
);

/// Folds a newer page (fetched from `newerCursor`) into the cached replies.
ThreadWindow mergeNewerThreadPage(ThreadWindow? cached, ThreadWindowPage page) {
  if (page.direction != WindowPageDirection.newer) {
    throw ArgumentError('Expected a newer thread page.');
  }
  final events = _sorted([
    ...(cached?.events ?? const <NostrEvent>[]),
    ...page.events,
  ]);
  final newerState = _newerState(page);
  return ThreadWindow(
    events: events,
    state: ThreadWindowState(
      hasOlder: cached?.state.hasOlder ?? false,
      olderCursor: cached?.state.olderCursor,
      newestCovered: _newestOf(events.where(_isReplyRow)),
      hasNewer: newerState.hasNewer,
      newerCursor: newerState.newerCursor,
    ),
  );
}

({bool hasNewer, ThreadWindowCursor? newerCursor}) _newerState(
  ThreadWindowPage page,
) => page.hasMore && page.nextCursor != null
    ? (hasNewer: true, newerCursor: page.nextCursor)
    : (hasNewer: false, newerCursor: null);

/// One reply window of a thread. Older windows run newest first from
/// [cursor] (the oldest reply already loaded), or from the newest reply when
/// null; newer windows run oldest first after [cursor], which they require.
NostrFilter threadWindowFilter({
  required String channelId,
  required String rootId,
  ThreadWindowCursor? cursor,
  WindowPageDirection direction = WindowPageDirection.older,
  int limit = threadWindowPageReplies,
}) {
  final newer = direction == WindowPageDirection.newer;
  if (newer && cursor == null) {
    throw ArgumentError('A newer thread window needs a cursor.');
  }
  return NostrFilter(
    kinds: threadWindowKinds,
    tags: {
      '#h': [channelId],
      '#e': [rootId],
    },
    limit: limit,
    until: newer ? null : cursor?.createdAt,
    since: newer ? cursor!.createdAt : null,
    // Rows only: reactions and edits reach thread rows through their own paths.
    extensions: {
      'thread_window': true,
      'include_aux': false,
      if (cursor != null) newer ? 'after_id' : 'before_id': cursor.eventId,
    },
  );
}

/// Reads one thread window out of a `/query` response: the events up to the
/// `39007` bounds overlay tagged with this thread. Returns null when the relay
/// served no window for it (no access, or a relay without thread windows).
/// Bounds that name a different [direction] than requested are rejected.
ThreadWindowPage? parseThreadWindowPage(
  List<NostrEvent> events, {
  required String channelId,
  required String rootId,
  WindowPageDirection direction = WindowPageDirection.older,
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
    final newer = direction == WindowPageDirection.newer;
    if (newer ? map['direction'] != 'newer' : map['direction'] == 'newer') {
      throw const FormatException('Thread window bounds run the wrong way.');
    }
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
      direction: direction,
    );
  }
  return null;
}
