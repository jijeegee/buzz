import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/message_db/coverage.dart';
import '../../shared/relay/relay.dart';
import 'channel_head_cache.dart';
import 'channel_room_store.dart';
import 'channel_window.dart';
import 'thread_replies_provider.dart';
import 'thread_window.dart';
import 'window_anchor.dart';

/// Writes fetched pages to the message database with the coverage each one
/// proves. Reads are unchanged: nothing here affects what a room shows.

CoverageCursor _cursorOf(NostrEvent event) =>
    CoverageCursor(event.createdAt, event.id);

/// The main-timeline coverage a parsed channel window page proves.
CoverageClaim? channelWindowPageClaim(
  String channelId,
  ChannelWindowPage page,
) {
  final start = page.startCursor;
  return windowPageClaim(
    channelId: channelId,
    stream: CoverageStream.main,
    rows: [for (final row in page.rows) _cursorOf(row.event)],
    start: start == null
        ? null
        : CoverageCursor(start.createdAt, start.eventId),
    newer: page.direction == WindowPageDirection.newer,
    hasMore: page.hasMore,
  );
}

/// Stores a channel window [response] (as the relay sent it) and the
/// coverage its parsed [page] proves.
void recordChannelWindowPage(
  Ref ref,
  String channelId,
  List<NostrEvent> response,
  ChannelWindowPage page,
) {
  final claim = channelWindowPageClaim(channelId, page);
  _recordPage(ref, channelId, response, claims: [?claim]);
}

/// The replies of [rootId] among a thread window's events: the thread's
/// conversation kinds only, so a root or another event riding along cannot
/// stretch the claimed range.
List<NostrEvent> _threadWindowRows(
  String rootId,
  Iterable<NostrEvent> events,
) => [
  for (final event in events)
    if (threadWindowKinds.contains(event.kind) &&
        event.threadReference.parentId != null &&
        event.threadReference.rootId == rootId)
      event,
];

/// Stores a thread window's [events] and the coverage they prove for the
/// thread-window stream. [hasMore] is null when the page's bounds are not at
/// hand (a preload): the rows then only prove the span between them.
void recordThreadWindowPage(
  Ref ref,
  ThreadRepliesArgs thread,
  List<NostrEvent> events, {
  ThreadWindowCursor? start,
  WindowPageDirection direction = WindowPageDirection.older,
  bool? hasMore,
}) {
  final claim = windowPageClaim(
    channelId: thread.channelId,
    stream: CoverageStream.threadWindow(thread.rootId),
    rows: _threadWindowRows(thread.rootId, events).map(_cursorOf),
    start: start == null
        ? null
        : CoverageCursor(start.createdAt, start.eventId),
    newer: direction == WindowPageDirection.newer,
    hasMore: hasMore ?? true,
  );
  _recordPage(ref, thread.channelId, events, claims: [?claim]);
}

/// Stores a complete scan of a thread's [replies]: every reply up to the
/// newest one's second, for both thread streams.
void recordCompleteThread(
  Ref ref,
  ThreadRepliesArgs thread,
  List<NostrEvent> replies,
) {
  final claims = completeThreadClaims(thread, replies);
  _recordPage(ref, thread.channelId, replies, claims: claims);
}

List<CoverageClaim> completeThreadClaims(
  ThreadRepliesArgs thread,
  List<NostrEvent> replies,
) {
  var newest = -1;
  for (final reply in replies) {
    if (reply.createdAt > newest) newest = reply.createdAt;
  }
  if (newest < 0) return const [];
  final range = CoverageRange(
    CoverageCursor.start,
    CoverageCursor.endOfSecond(newest),
  );
  return [
    for (final stream in [
      CoverageStream.thread(thread.rootId),
      CoverageStream.threadWindow(thread.rootId),
    ])
      CoverageClaim(channelId: thread.channelId, stream: stream, range: range),
  ];
}

/// Stores events loaded without a coverage claim (by id, or from the legacy
/// websocket history).
void recordLoadedEvents(Ref ref, Iterable<NostrEvent> events) {
  final scope = _scope(ref);
  if (scope == null) return;
  try {
    ref.read(channelRoomStoreProvider).record(scope, events);
  } catch (error) {
    debugPrint('[ChannelPageRecords] failed to queue events: $error');
  }
}

void _recordPage(
  Ref ref,
  String channelId,
  List<NostrEvent> events, {
  required List<CoverageClaim> claims,
}) {
  final scope = _scope(ref);
  if (scope == null) return;
  try {
    unawaited(
      ref
          .read(channelRoomStoreProvider)
          .recordPage(scope, events, channelId: channelId, claims: claims),
    );
  } catch (error) {
    debugPrint('[ChannelPageRecords] failed to store a page: $error');
  }
}

ChannelHeadScope? _scope(Ref ref) {
  try {
    return ref.read(channelHeadScopeProvider);
  } catch (_) {
    // A disposed provider's ref; the page is not stored.
    return null;
  }
}
