import 'package:flutter/foundation.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/relay/relay.dart';
import 'channel_head_cache.dart';
import 'channel_messages_provider.dart';
import 'channel_window.dart';
import 'store_preload_plan.dart';
import 'thread_replies_provider.dart';

/// Newest messages loaded per preloaded room: a whole open window, so its
/// answer can vouch for it (docs/message-sync.md#head-spans).
const channelPreloadMessages = 50;

/// Newest replies loaded per preloaded thread, about one thread screen.
const threadPreloadReplies = 20;

/// Rooms one batched window request carries.
const maxChannelWindowsPerRequest = 12;

/// The relay serves at most four thread windows per `/query`.
const maxThreadWindowsPerRequest = 4;

/// A room or thread that failed or was skipped is not retried sooner.
const preloadRetryAfter = Duration(minutes: 5);

/// A room drawn from the store is looked at again after this.
const certainRetryAfter = Duration(seconds: 60);

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
) {
  final wanted = {
    for (final channelId in channelIds) channelId.toLowerCase(): channelId,
  };
  final windows = <String, List<NostrEvent>>{};
  var segment = <NostrEvent>[];
  for (final event in events) {
    segment.add(event);
    if (event.kind != EventKind.channelWindowBounds) continue;
    final channelId = wanted[event.getTagValue('h')?.toLowerCase()];
    final window = segment;
    segment = [];
    if (channelId != null) windows[channelId] = window;
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

/// The store side of a preload pass: [live] waits (at most 3 s) for the
/// rooms' live REQs to settle and returns those with a token; [certain]
/// returns a room's newest stored window (channel-window response) when it
/// is certain, else null.
typedef StorePreloadBridge = ({
  Future<Set<String>> Function(List<String> rooms) live,
  Future<List<NostrEvent>?> Function(String room) certain,
});

/// Loads the newest page of rooms and threads in the background so opening
/// them paints at once. Rooms are written to the persisted [ChannelHeadCache],
/// which an opening room paints before its own relay page replaces it; no
/// room notifier is created. Threads are kept in [preloadedThreadsProvider],
/// which an opening thread shows only until its full scan lands.
class ChannelPreloader {
  ChannelPreloader(
    this._ref, {
    DateTime Function()? now,
    StorePreloadBridge? store,
  }) : _now = now ?? DateTime.now,
       _store = store;

  final Ref _ref;
  final DateTime Function() _now;
  final StorePreloadBridge? _store;
  final Map<String, DateTime> _attemptedAt = {};

  /// Whether a background request may go out now: the session is connected
  /// and the relay is not pushing back.
  bool get canRequest {
    if (_ref.read(relaySessionProvider).status != SessionStatus.connected) {
      return false;
    }
    return !_ref.read(relaySessionProvider.notifier).isRateLimited;
  }

  StorePreloadBridge get _storeBridge {
    if (_store case final store?) return store;
    final session = _ref.read(relaySessionProvider.notifier);
    final filter = channelPreloadFilter('');
    return (
      live: (rooms) => session.waitStoreLiveRooms(rooms),
      certain: (room) => session.certainRoomHead(
        room,
        kinds: filter.kinds,
        limit: filter.limit,
      ),
    );
  }

  /// Of [rooms], those that need the relay: once their live REQs have
  /// settled (at most 3 s), rooms whose newest stored window is certain are
  /// left out and not marked fetched, so opening them draws from the store
  /// and asks the relay once; their stored window is written as the
  /// persisted head (theirs was missing or older than their newest
  /// message), so the open paints it at once
  /// (docs/message-sync.md#store-first-preload).
  Future<List<String>> _roomsNeedingRelay(
    List<String> rooms,
    ChannelHeadScope scope,
  ) async {
    final store = _storeBridge;
    final tokens = await store.live(rooms);
    // A room opened meanwhile loads itself.
    final cold = [
      for (final room in rooms)
        if (!_ref.exists(channelMessagesProvider(room))) room,
    ];
    final check = planStorePreload(cold, tokens, const {}).check;
    final windows = <String, List<NostrEvent>>{};
    for (final room in check) {
      try {
        if (await store.certain(room) case final window?) {
          windows[room] = window;
        }
      } catch (_) {
        // Unreadable: the relay is asked.
      }
    }
    final plan = planStorePreload(cold, tokens, windows.keys.toSet());
    debugPrint(storePreloadSummary(plan));
    if (_ref.read(channelHeadScopeProvider) != scope) return const [];
    final cache = _ref.read(channelHeadCacheProvider);
    final retryAt = _now().subtract(preloadRetryAfter - certainRetryAfter);
    for (final room in plan.certain) {
      _attemptedAt['room:$room'] = retryAt;
      // A room opened meanwhile persists its own page.
      if (_ref.exists(channelMessagesProvider(room))) continue;
      await cache.store(scope, room, windows[room]!);
    }
    return plan.fetch;
  }

  bool _recentlyAttempted(String key) {
    final at = _attemptedAt[key];
    return at != null && _now().difference(at) < preloadRetryAfter;
  }

  /// Loads the newest window of up to [maxChannelWindowsPerRequest] rooms in
  /// one request. Skips rooms that are open or loaded in memory, tried within
  /// [preloadRetryAfter], whose persisted head was written after their
  /// newest message, or whose newest window the store holds certainly
  /// ([_roomsNeedingRelay]). Resolves to the rooms stored.
  ///
  /// [beforeFetch] runs right before the relay request, after the store
  /// check, and skips the request when it resolves to false.
  Future<List<String>> preloadRooms(
    List<ChannelPreloadTarget> targets, {
    Future<bool> Function()? beforeFetch,
  }) async {
    final scope = _ref.read(channelHeadScopeProvider);
    if (scope == null) return const [];
    final cache = _ref.read(channelHeadCacheProvider);
    final cold = <String>[];
    for (final target in targets) {
      final channelId = target.channelId;
      if (cold.length == maxChannelWindowsPerRequest) break;
      if (cold.contains(channelId) ||
          _recentlyAttempted('room:$channelId') ||
          _ref.exists(channelMessagesProvider(channelId))) {
        continue;
      }
      final storedAt = await cache.storedAt(scope, channelId);
      if (storedAt != null &&
          storedAt.millisecondsSinceEpoch ~/ 1000 >= target.latestAt) {
        continue;
      }
      cold.add(channelId);
    }
    if (cold.isEmpty) return const [];
    final attemptedAt = _now();
    for (final channelId in cold) {
      _attemptedAt['room:$channelId'] = attemptedAt;
    }
    final fetch = await _roomsNeedingRelay(cold, scope);
    if (fetch.isEmpty || _ref.read(channelHeadScopeProvider) != scope) {
      return const [];
    }
    if (beforeFetch != null && !await beforeFetch()) return const [];
    if (_ref.read(channelHeadScopeProvider) != scope) return const [];

    final List<NostrEvent> events;
    try {
      events = await _ref
          .read(relaySessionProvider.notifier)
          .queryRelayWithStoreHeads([
            for (final channelId in fetch) channelPreloadFilter(channelId),
          ]);
    } catch (error) {
      // The scheduler tries again in a later pass.
      debugPrint('[ChannelPreloader] batched room preload failed: $error');
      return const [];
    }
    if (_ref.read(channelHeadScopeProvider) != scope) return const [];
    final stored = <String>[];
    for (final MapEntry(key: channelId, value: window) in splitChannelWindows(
      events,
      fetch,
    ).entries) {
      try {
        parseChannelWindowResponse(window, channelId, null);
      } catch (error) {
        debugPrint('[ChannelPreloader] invalid window for $channelId: $error');
        continue;
      }
      // A room opened meanwhile persists its own, newer page.
      if (_ref.exists(channelMessagesProvider(channelId))) continue;
      await cache.store(scope, channelId, window);
      _ref.read(relayFetchedRoomsProvider).add(channelId);
      stored.add(channelId);
    }
    return stored;
  }

  /// Loads the newest replies of threads, [maxThreadWindowsPerRequest] per
  /// request. Skips threads whose replies are already in memory or that were
  /// tried within [preloadRetryAfter]. [beforeRequest] runs before each
  /// request and stops the preload when it resolves to false.
  Future<int> preloadThreads(
    List<ThreadRepliesArgs> threads, {
    Future<bool> Function()? beforeRequest,
  }) async {
    final cold = <ThreadRepliesArgs>[
      for (final thread in threads.toSet())
        if (!_recentlyAttempted('thread:${thread.rootId}') &&
            !_ref.exists(threadRepliesProvider(thread)))
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
        final events = await _ref
            .read(relaySessionProvider.notifier)
            .queryRelayWithStoreHeads([
              for (final thread in chunk) threadPreloadFilter(thread),
            ]);
        final windows = splitThreadWindows(events, chunk);
        final store = _ref.read(preloadedThreadsProvider.notifier);
        for (final MapEntry(key: thread, value: replies) in windows.entries) {
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
