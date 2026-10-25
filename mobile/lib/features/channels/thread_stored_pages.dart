import 'package:flutter/foundation.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/message_db/coverage.dart';
import '../../shared/message_db/message_reads.dart';
import '../../shared/message_db/message_store.dart';
import '../../shared/relay/relay.dart';
import 'channel_room_store.dart';
import 'thread_replies_provider.dart';
import 'thread_window.dart';
import 'window_anchor.dart';

/// A thread's replies read from the message database, in the shape of the
/// thread window pages the relay serves. Reads are synchronous (the UI
/// isolate's read connection), so a thread can paint its first frame from
/// them, and they return only rows the database knows complete.

/// The newest stored page of a thread, and whether it needs nothing from the
/// server.
typedef StoredThreadOpen = ({ThreadWindowPage page, bool covered});

MessageScope? _scope(Ref ref) {
  try {
    return ref.read(messageScopeProvider);
  } catch (_) {
    return null;
  }
}

ChannelRoomStore? _store(Ref ref) {
  try {
    return ref.read(channelRoomStoreProvider);
  } catch (_) {
    return null;
  }
}

CoverageCursor _cursor(ThreadWindowCursor cursor) =>
    CoverageCursor(cursor.createdAt, cursor.eventId);

/// The newest stored replies of [thread], with the events of it still
/// queued for the database. Covered when the stored replies run on into the
/// room's open live range while the channel list's live feed covers the
/// room and the relay is connected: then nothing newer is missing. Null when
/// nothing complete is stored.
StoredThreadOpen? readStoredNewestThreadPage(
  Ref ref,
  ThreadRepliesArgs thread,
) {
  final scope = _scope(ref);
  final store = _store(ref);
  if (scope == null || store == null) return null;
  try {
    final stored = store.readNewest(
      scope,
      thread.channelId,
      rootId: thread.rootId,
    );
    if (stored == null) return null;
    final page = _olderPage(stored);
    final covered =
        stored.reachesLive &&
        ref.read(relaySessionProvider).status == SessionStatus.connected &&
        store.feeds(scope, thread.channelId) &&
        store.hasOpenLiveRange(scope, thread.channelId);
    final queued = _queuedThreadEvents(
      store.unflushed(scope, thread.channelId),
      thread.rootId,
      stored: page.events,
      storedRows: stored.rows,
    );
    return (
      page: queued.isEmpty
          ? page
          : ThreadWindowPage(
              events: [...page.events, ...queued],
              hasMore: page.hasMore,
              nextCursor: page.nextCursor,
            ),
      covered: covered,
    );
  } catch (error) {
    debugPrint('[ThreadStoredPages] stored replies unusable: $error');
    return null;
  }
}

/// The stored replies just older than [cursor], when known complete.
ThreadWindowPage? readStoredOlderThreadPage(
  Ref ref,
  ThreadRepliesArgs thread,
  ThreadWindowCursor cursor,
) {
  final scope = _scope(ref);
  final store = _store(ref);
  if (scope == null || store == null) return null;
  try {
    final stored = store.readOlder(
      scope,
      thread.channelId,
      _cursor(cursor),
      rootId: thread.rootId,
    );
    return stored == null ? null : _olderPage(stored);
  } catch (error) {
    debugPrint('[ThreadStoredPages] stored older replies unusable: $error');
    return null;
  }
}

/// The stored replies just newer than [cursor], oldest first, when known
/// complete.
ThreadWindowPage? readStoredNewerThreadPage(
  Ref ref,
  ThreadRepliesArgs thread,
  ThreadWindowCursor cursor,
) {
  final scope = _scope(ref);
  final store = _store(ref);
  if (scope == null || store == null) return null;
  try {
    final stored = store.readNewer(
      scope,
      thread.channelId,
      _cursor(cursor),
      rootId: thread.rootId,
    );
    if (stored == null) return null;
    final hasMore = !stored.reachesLive && stored.rows.isNotEmpty;
    return ThreadWindowPage(
      events: [...stored.rows, ...stored.overlays],
      hasMore: hasMore,
      nextCursor: hasMore ? ThreadWindowCursor.of(stored.rows.last) : null,
      direction: WindowPageDirection.newer,
    );
  } catch (error) {
    debugPrint('[ThreadStoredPages] stored newer replies unusable: $error');
    return null;
  }
}

/// The window opened at an anchor (see [windowAnchorCursor]) from stored
/// replies, when both sides of it are known complete.
({ThreadWindowPage older, ThreadWindowPage newer})? readStoredThreadMiddle(
  Ref ref,
  ThreadRepliesArgs thread,
  ThreadWindowCursor anchor,
) {
  final older = readStoredOlderThreadPage(ref, thread, anchor);
  if (older == null) return null;
  final newer = readStoredNewerThreadPage(ref, thread, anchor);
  if (newer == null) return null;
  return (older: older, newer: newer);
}

/// [stored] as the older-direction page the relay would have served.
ThreadWindowPage _olderPage(StoredTimelinePage stored) {
  final hasMore = !stored.reachesStart && stored.rows.isNotEmpty;
  return ThreadWindowPage(
    events: [...stored.rows, ...stored.overlays],
    hasMore: hasMore,
    nextCursor: hasMore ? ThreadWindowCursor.of(stored.rows.last) : null,
  );
}

/// The queued events that belong to the thread: its replies, and edits,
/// reactions and deletions of them. [stored] are the events already read,
/// [storedRows] the replies among them.
List<NostrEvent> _queuedThreadEvents(
  List<NostrEvent> queued,
  String rootId, {
  required List<NostrEvent> stored,
  required List<NostrEvent> storedRows,
}) {
  final storedIds = {for (final event in stored) event.id};
  final rows = [
    for (final event in queued)
      if (EventKind.channelTimelineContentKinds.contains(event.kind) &&
          event.threadReference.parentId != null &&
          event.threadReference.rootId == rootId &&
          !storedIds.contains(event.id))
        event,
  ];
  final rowIds = {
    for (final event in storedRows) event.id,
    for (final event in rows) event.id,
  };
  return [
    ...rows,
    for (final event in queued)
      if ((event.kind == EventKind.reaction ||
              event.kind == EventKind.streamMessageEdit ||
              event.kind == EventKind.deletion ||
              event.kind == EventKind.nip29DeleteEvent) &&
          !storedIds.contains(event.id) &&
          event.tags.any(
            (tag) => tag.length > 1 && tag[0] == 'e' && rowIds.contains(tag[1]),
          ))
        event,
  ];
}

/// The thread replies among [events], for the room's reply cache.
List<NostrEvent> storedThreadReplies(
  ThreadRepliesArgs thread,
  Iterable<NostrEvent> events,
) => [
  for (final event in events)
    if (EventKind.channelTimelineContentKinds.contains(event.kind) &&
        event.channelId == thread.channelId &&
        event.threadReference.parentId != null &&
        event.threadReference.rootId == thread.rootId)
      event,
];
