import 'package:flutter/foundation.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/message_db/coverage.dart';
import '../../shared/read_state/read_state_provider.dart';
import '../../shared/relay/relay.dart';
import '../../shared/message_db/message_store.dart';
import 'channel_messages_provider.dart';
import 'channel_page_records.dart';
import 'channel_room_store.dart';
import 'channel_window.dart';
import 'channels_provider.dart';
import 'thread_replies_provider.dart';
import 'thread_stored_pages.dart';
import 'window_anchor.dart';

/// Newest messages loaded per preloaded room, about one chat screen.
const channelPreloadMessages = 20;

/// Newest replies loaded per preloaded thread, about one thread screen.
const threadPreloadReplies = 20;

/// Rooms one batched window request carries.
const maxChannelWindowsPerRequest = 12;

/// The relay serves at most four thread windows per `/query`.
const maxThreadWindowsPerRequest = 4;

/// Rooms with unread messages whose read marker window is preloaded.
const maxReadMarkerRooms = 20;

/// A room or thread that failed or was skipped is not retried sooner.
const preloadRetryAfter = Duration(minutes: 5);

/// Kind of the relay-signed overlay that closes every thread window.
const _threadWindowBoundsKind = 39007;

/// Conversation kinds a thread window pages (NIP-CW thread mode).
const _threadWindowKinds = [EventKind.streamMessage, EventKind.streamMessageV2];

/// The newest-window filter of one room, as the room itself requests it but
/// with a smaller page.
NostrFilter channelPreloadFilter(String channelId) => NostrFilter(
  kinds: EventKind.channelTimelineContentKinds,
  tags: {
    '#h': [channelId],
  },
  limit: channelPreloadMessages,
  extensions: const {
    'top_level': true,
    'include_summaries': true,
    'include_aux': true,
  },
);

/// The newest-first reply window of one thread.
NostrFilter threadPreloadFilter(ThreadRepliesArgs thread) => NostrFilter(
  kinds: _threadWindowKinds,
  tags: {
    '#h': [thread.channelId],
    '#e': [thread.rootId],
  },
  limit: threadPreloadReplies,
  // Rows only: reactions and edits reach thread rows through their own paths.
  extensions: const {'thread_window': true, 'include_aux': false},
);

/// Splits one multi-window `/query` response into per-room windows.
///
/// The relay serves window filters in request order and closes each window
/// with exactly one `39006` bounds overlay tagged with its room; a room the
/// reader cannot see emits nothing. So every run of events up to a bounds
/// overlay belongs to that overlay's room. Trailing events with no closing
/// overlay, and rooms not asked for, are dropped. Mirrors desktop's
/// `split_channel_windows`.
Map<String, List<NostrEvent>> splitChannelWindows(
  List<NostrEvent> events,
  Iterable<String> channelIds,
) => {
  for (final (channelId, window) in splitChannelWindowRuns(events, channelIds))
    channelId: window,
};

/// [splitChannelWindows] keeping every window in response order, for
/// requests that ask one room for more than one window.
List<(String, List<NostrEvent>)> splitChannelWindowRuns(
  List<NostrEvent> events,
  Iterable<String> channelIds,
) {
  final wanted = {
    for (final channelId in channelIds) channelId.toLowerCase(): channelId,
  };
  final windows = <(String, List<NostrEvent>)>[];
  var segment = <NostrEvent>[];
  for (final event in events) {
    segment.add(event);
    if (event.kind != EventKind.channelWindowBounds) continue;
    final channelId = wanted[event.getTagValue('h')?.toLowerCase()];
    final window = segment;
    segment = [];
    if (channelId != null) windows.add((channelId, window));
  }
  return windows;
}

/// Splits one multi-thread `/query` response into per-thread replies, at each
/// `39007` bounds overlay tagged with its room (`h`) and root (`e`). The
/// overlays themselves are dropped. Mirrors desktop's `split_thread_windows`.
Map<ThreadRepliesArgs, List<NostrEvent>> splitThreadWindows(
  List<NostrEvent> events,
  Iterable<ThreadRepliesArgs> threads,
) {
  final wanted = threads.toSet();
  final windows = <ThreadRepliesArgs, List<NostrEvent>>{};
  var segment = <NostrEvent>[];
  for (final event in events) {
    if (event.kind != _threadWindowBoundsKind) {
      segment.add(event);
      continue;
    }
    final window = segment;
    segment = [];
    final channelId = event.getTagValue('h');
    final rootId = event.getTagValue('e');
    if (channelId == null || rootId == null) continue;
    final thread = ThreadRepliesArgs(channelId: channelId, rootId: rootId);
    if (wanted.contains(thread)) windows[thread] = window;
  }
  return windows;
}

/// A room to preload and the time of its newest known message.
typedef ChannelPreloadTarget = ({String channelId, int latestAt});

/// Loads the newest page of rooms and threads in the background so opening
/// them paints at once. Rooms are written to the message database, which an
/// opening room paints from; no room notifier is created. Threads are kept
/// in [preloadedThreadsProvider], which an opening thread shows only until
/// its full scan lands.
class ChannelPreloader {
  ChannelPreloader(this._ref, {DateTime Function()? now})
    : _now = now ?? DateTime.now;

  final Ref _ref;
  final DateTime Function() _now;
  final Map<String, DateTime> _attemptedAt = {};

  /// Whether a background request may go out now: the session is connected
  /// and the relay is not pushing back.
  bool get canRequest {
    if (_ref.read(relaySessionProvider).status != SessionStatus.connected) {
      return false;
    }
    return !_ref.read(relaySessionProvider.notifier).isRateLimited;
  }

  bool _recentlyAttempted(String key) {
    final at = _attemptedAt[key];
    return at != null && _now().difference(at) < preloadRetryAfter;
  }

  /// Loads the newest window of up to [maxChannelWindowsPerRequest] rooms in
  /// one request. Skips rooms that are open or loaded in memory, tried within
  /// [preloadRetryAfter], or stored through their newest message. Resolves
  /// to the rooms stored.
  Future<List<String>> preloadRooms(List<ChannelPreloadTarget> targets) async {
    final scope = _ref.read(messageScopeProvider);
    if (scope == null) return const [];
    final store = _ref.read(channelRoomStoreProvider);
    final cold = <String>[];
    for (final target in targets) {
      final channelId = target.channelId;
      if (cold.length == maxChannelWindowsPerRequest) break;
      if (cold.contains(channelId) ||
          _recentlyAttempted('room:$channelId') ||
          _ref.exists(channelMessagesProvider(channelId))) {
        continue;
      }
      if (store.isStoredThrough(scope, channelId, target.latestAt)) continue;
      cold.add(channelId);
    }
    if (cold.isEmpty) return const [];
    final attemptedAt = _now();
    for (final channelId in cold) {
      _attemptedAt['room:$channelId'] = attemptedAt;
    }

    final List<NostrEvent> events;
    try {
      events = await _ref.read(relaySessionProvider.notifier).queryRelay([
        for (final channelId in cold) channelPreloadFilter(channelId),
      ]);
    } catch (error) {
      // The scheduler tries again in a later pass.
      debugPrint('[ChannelPreloader] batched room preload failed: $error');
      return const [];
    }
    if (_ref.read(messageScopeProvider) != scope) return const [];
    final stored = <String>[];
    final writes = <Future<void>>[];
    for (final MapEntry(key: channelId, value: window) in splitChannelWindows(
      events,
      cold,
    ).entries) {
      final ChannelWindowPage page;
      try {
        page = parseChannelWindowResponse(window, channelId, null);
      } catch (error) {
        debugPrint('[ChannelPreloader] invalid window for $channelId: $error');
        continue;
      }
      writes.add(recordChannelWindowPage(_ref, channelId, window, page));
      // The newest rows are stored: a room the live feed covers stays
      // complete from them on.
      store.clearGap(
        scope,
        channelId,
        at: page.rows.firstOrNull?.event.createdAt,
      );
      stored.add(channelId);
    }
    await Future.wait(writes);
    return stored;
  }

  /// Loads the window around the read marker of up to [maxReadMarkerRooms]
  /// rooms with unread messages, newest unread first, so a room opening at
  /// its first unread message paints it at once: the older and the newer
  /// page from the marker, [maxChannelWindowsPerRequest] windows a request.
  /// Skips rooms that are open or loaded in memory, tried within
  /// [preloadRetryAfter], or stored around their marker already.
  /// [beforeRequest] as in [preloadThreads]. Resolves to the rooms stored.
  Future<int> preloadReadMarkers({
    Future<bool> Function()? beforeRequest,
  }) async {
    final scope = _ref.read(messageScopeProvider);
    if (scope == null) return 0;
    final targets = _readMarkerTargets(scope);
    var loaded = 0;
    const roomsPerRequest = maxChannelWindowsPerRequest ~/ 2;
    for (var start = 0; start < targets.length; start += roomsPerRequest) {
      if (beforeRequest != null && !await beforeRequest()) return loaded;
      final chunk = targets.sublist(
        start,
        (start + roomsPerRequest).clamp(0, targets.length),
      );
      final cursors = <String, ChannelPageCursor>{};
      final attemptedAt = _now();
      for (final (channelId, readAt) in chunk) {
        _attemptedAt['marker:$channelId'] = attemptedAt;
        final anchor = windowAnchorCursor(createdAt: readAt);
        cursors[channelId] = ChannelPageCursor(
          createdAt: anchor.createdAt,
          eventId: anchor.eventId,
        );
      }
      final List<NostrEvent> events;
      try {
        events = await _ref.read(relaySessionProvider.notifier).queryRelay([
          for (final MapEntry(key: channelId, value: cursor) in cursors.entries)
            for (final direction in WindowPageDirection.values)
              channelWindowFilter(channelId, cursor, direction: direction),
        ]);
      } catch (error) {
        debugPrint('[ChannelPreloader] read marker preload failed: $error');
        continue;
      }
      if (_ref.read(messageScopeProvider) != scope) return loaded;
      final stored = <String>{};
      final writes = <Future<void>>[];
      for (final (channelId, window) in splitChannelWindowRuns(
        events,
        cursors.keys,
      )) {
        final page = _parseMarkerWindow(window, channelId, cursors[channelId]!);
        if (page == null) continue;
        writes.add(recordChannelWindowPage(_ref, channelId, window, page));
        stored.add(channelId);
      }
      await Future.wait(writes);
      loaded += stored.length;
    }
    return loaded;
  }

  /// The rooms [preloadReadMarkers] loads, with their read markers.
  List<(String, int)> _readMarkerTargets(MessageScope scope) {
    final readState = _ref.read(readStateProvider);
    final unread = _ref
        .read(channelsProvider.notifier)
        .observedUnreadEventsByChannel;
    final forums = {
      for (final channel in _ref.read(channelsProvider).value ?? const [])
        if (channel.isForum) channel.id,
    };
    final store = _ref.read(channelRoomStoreProvider);
    final rooms = <(String, int, int)>[];
    for (final MapEntry(key: channelId, value: events) in unread.entries) {
      final readAt = readState.effectiveTimestamp(channelId);
      if (readAt == null ||
          forums.contains(channelId) ||
          _recentlyAttempted('marker:$channelId') ||
          _ref.exists(channelMessagesProvider(channelId))) {
        continue;
      }
      var newest = 0;
      for (final event in events.values) {
        if (event.rootId == null && event.createdAt > newest) {
          newest = event.createdAt;
        }
      }
      if (newest <= readAt) continue;
      final at = CoverageCursor(readAt, zeroEventId);
      try {
        if (store.readOlder(scope, channelId, at) != null &&
            store.readNewer(scope, channelId, at) != null) {
          continue;
        }
      } catch (_) {
        // Unreadable here: load it.
      }
      rooms.add((channelId, readAt, newest));
    }
    rooms.sort((a, b) => b.$3.compareTo(a.$3));
    return [
      for (final (channelId, readAt, _) in rooms.take(maxReadMarkerRooms))
        (channelId, readAt),
    ];
  }

  /// One window of a read marker request, whichever way it runs.
  ChannelWindowPage? _parseMarkerWindow(
    List<NostrEvent> window,
    String channelId,
    ChannelPageCursor cursor,
  ) {
    for (final direction in WindowPageDirection.values) {
      try {
        return parseChannelWindowResponse(
          window,
          channelId,
          cursor,
          direction: direction,
        );
      } catch (_) {
        // The other direction, or invalid.
      }
    }
    debugPrint('[ChannelPreloader] invalid marker window for $channelId');
    return null;
  }

  /// Loads the newest replies of threads, [maxThreadWindowsPerRequest] per
  /// request. Skips threads whose replies are already in memory or stored
  /// complete up to now, or that were tried within [preloadRetryAfter].
  /// [beforeRequest] runs before each request and stops the preload when it
  /// resolves to false.
  Future<int> preloadThreads(
    List<ThreadRepliesArgs> threads, {
    Future<bool> Function()? beforeRequest,
  }) async {
    final cold = <ThreadRepliesArgs>[
      for (final thread in threads.toSet())
        if (!_recentlyAttempted('thread:${thread.rootId}') &&
            !_ref.exists(threadRepliesProvider(thread)) &&
            readStoredNewestThreadPage(_ref, thread)?.covered != true)
          thread,
    ];
    var loaded = 0;
    for (
      var start = 0;
      start < cold.length;
      start += maxThreadWindowsPerRequest
    ) {
      if (beforeRequest != null && !await beforeRequest()) return loaded;
      final chunk = cold.sublist(
        start,
        (start + maxThreadWindowsPerRequest).clamp(0, cold.length),
      );
      final attemptedAt = _now();
      for (final thread in chunk) {
        _attemptedAt['thread:${thread.rootId}'] = attemptedAt;
      }
      try {
        final liveBefore = {
          for (final thread in chunk)
            thread.channelId: fedLiveFrom(_ref, thread.channelId),
        };
        final events = await _ref
            .read(relaySessionProvider.notifier)
            .queryRelay([
              for (final thread in chunk) threadPreloadFilter(thread),
            ]);
        final windows = splitThreadWindows(events, chunk);
        final store = _ref.read(preloadedThreadsProvider.notifier);
        for (final MapEntry(key: thread, value: replies) in windows.entries) {
          final liveFrom = liveBefore[thread.channelId];
          recordThreadWindowPage(
            _ref,
            thread,
            replies,
            liveFrom:
                liveFrom != null &&
                    liveFrom == fedLiveFrom(_ref, thread.channelId)
                ? liveFrom
                : null,
          );
          // A thread opened meanwhile has its full scan.
          if (_ref.exists(threadRepliesProvider(thread))) continue;
          store.seed(thread, replies);
          loaded++;
        }
      } catch (error) {
        debugPrint('[ChannelPreloader] thread preload failed: $error');
      }
    }
    return loaded;
  }
}

final channelPreloaderProvider = Provider<ChannelPreloader>(
  ChannelPreloader.new,
);

/// The newest replies preloaded for a thread, with when they were loaded.
typedef PreloadedThread = ({List<NostrEvent> replies, DateTime loadedAt});

/// Preloaded newest replies per thread. An opening thread shows them with its
/// route snapshot until its own full scan lands; they never stand in for that
/// scan (a partial page proves nothing about deleted replies). Bounded to the
/// most recently loaded [maxEntries] threads, each kept for
/// [threadRepliesRetention].
class PreloadedThreadsNotifier
    extends Notifier<Map<ThreadRepliesArgs, PreloadedThread>> {
  PreloadedThreadsNotifier({DateTime Function()? now})
    : _now = now ?? DateTime.now;

  static const maxEntries = 64;

  final DateTime Function() _now;

  @override
  Map<ThreadRepliesArgs, PreloadedThread> build() => const {};

  void seed(ThreadRepliesArgs thread, List<NostrEvent> replies) {
    final now = _now();
    final kept = <ThreadRepliesArgs, PreloadedThread>{
      for (final entry in state.entries)
        if (entry.key != thread &&
            now.difference(entry.value.loadedAt) < threadRepliesRetention)
          entry.key: entry.value,
      thread: (replies: List.unmodifiable(replies), loadedAt: now),
    };
    // Map literals keep insertion order: the oldest seeds come first.
    final entries = kept.entries.toList();
    state = Map.unmodifiable(
      Map.fromEntries(
        entries.length > maxEntries
            ? entries.sublist(entries.length - maxEntries)
            : entries,
      ),
    );
  }

  /// The preloaded replies of [thread], if still fresh.
  List<NostrEvent> repliesOf(ThreadRepliesArgs thread) {
    final entry = state[thread];
    if (entry == null ||
        _now().difference(entry.loadedAt) >= threadRepliesRetention) {
      return const [];
    }
    return entry.replies;
  }
}

final preloadedThreadsProvider =
    NotifierProvider<
      PreloadedThreadsNotifier,
      Map<ThreadRepliesArgs, PreloadedThread>
    >(PreloadedThreadsNotifier.new);

/// The preloaded newest replies of one thread, empty when none are fresh.
final preloadedThreadRepliesProvider = Provider.autoDispose
    .family<List<NostrEvent>, ThreadRepliesArgs>((ref, thread) {
      ref.watch(preloadedThreadsProvider.select((threads) => threads[thread]));
      return ref.read(preloadedThreadsProvider.notifier).repliesOf(thread);
    });
