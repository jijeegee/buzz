import 'dart:async';
import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/read_state/read_state_provider.dart';
import '../../shared/relay/relay.dart';
import '../../shared/theme/theme_provider.dart';
import '../channels/channel_live_events.dart';
import 'chat_list.dart';
import 'chat_state_writer.dart';
import 'thread_pins_provider.dart';

/// Ephemeral kind the relay sends to all of a user's devices when their chat
/// state changes (a read, unread mark or follow on another device).
const chatStatePingKind = 24210;

/// Shortest time between reloads triggered by app resume, pings or deletions.
/// Reconnects and pull to refresh always reload.
final chatListMinReloadIntervalProvider = Provider<Duration>(
  (ref) => const Duration(seconds: 30),
);

String _ownerOf(RelayConfig config, String pubkey) =>
    '${config.baseUrl}:${pubkey.toLowerCase()}';

String _cacheKey(RelayConfig config, String pubkey) =>
    'kuzz.chatList.v1:${_ownerOf(config, pubkey)}';

/// A first page that failed for a passing reason is retried this soon.
const _firstPageRetryDelay = Duration(seconds: 5);

/// How long one `chat_list` page may take: desktop's `/query` deadline.
/// Longer than the phone's other queries (8s), so a slow relay still fills
/// the list with previews; the screen shows this device's rooms meanwhile.
const chatListQueryTimeout = Duration(seconds: 30);

/// The Chats list from the relay's `chat_list`: rooms and live threads,
/// newest first, with server unread counts. The last first page is saved on
/// this device and drawn at once on the next start. Live messages patch only
/// their row; rows only the relay can place, chat-state pings from other
/// devices, reconnects and app resume reload the first page. Mirrors
/// desktop's `useServerChatList`.
///
/// An error with no value means the relay keeps no chat state; the Chats
/// screen then shows the channel list instead.
///
/// One list per [owner] (relay and account), so another account's rows are
/// never shown or merged.
class ChatListNotifier extends AsyncNotifier<ChatListState> {
  ChatListNotifier(this.owner);

  final String owner;

  int _generation = 0;
  List<NostrEvent>? _eventsDuringLoad;
  DateTime _loadedAt = DateTime.fromMillisecondsSinceEpoch(0);
  Timer? _reloadTimer;
  bool _backgrounded = false;
  bool _firstPageInFlight = false;
  bool _pinsChangedDuringLoad = false;

  @override
  Future<ChatListState> build() async {
    final config = ref.watch(relayConfigProvider);
    final pubkey = ref.watch(myPubkeyProvider);
    final connected = ref.watch(
      relaySessionProvider.select((s) => s.status == SessionStatus.connected),
    );
    final generation = ++_generation;
    ref.onDispose(() {
      _generation++;
      _reloadTimer?.cancel();
      _reloadTimer = null;
    });
    if (pubkey == null || pubkey.isEmpty || _ownerOf(config, pubkey) != owner) {
      return const ChatListState(rows: [], next: null);
    }
    final cached = state.value ?? _readCache(config, pubkey);
    if (!connected) {
      // Keep (or show the saved) rows until the session reconnects, which
      // rebuilds this provider and reloads the list.
      if (cached != null) return cached;
      return Completer<ChatListState>().future;
    }
    ref.onDispose(
      ref
          .read(channelLiveEventsProvider)
          .listen((event) => _handleLiveEvent(event, generation)),
    );
    ref.listen(appLifecycleProvider, (_, next) {
      if (next == AppLifecycleState.paused ||
          next == AppLifecycleState.hidden) {
        _backgrounded = true;
      } else if (next == AppLifecycleState.resumed && _backgrounded) {
        _backgrounded = false;
        _reloadSoon(generation);
      }
    });
    unawaited(_subscribePing(pubkey, generation));
    // The relay keeps pinned threads listed while quiet or closed; a changed
    // pin set reloads the list with it.
    ref.listen(threadPinsProvider, (previous, next) {
      // Any change reloads, as desktop (the pins are part of its query key).
      if (setEquals(previous, next)) return;
      // Never drop a first page still loading (it can take a while on a
      // slow relay): reload once it is done.
      if (_firstPageInFlight) {
        _pinsChangedDuringLoad = true;
        return;
      }
      _reloadSoon(generation, immediate: true);
    });

    if (cached != null && !state.hasValue) {
      // Draw the saved copy at once; the first page replaces it.
      Timer.run(() => unawaited(_replaceCache(config, pubkey, generation)));
      return cached;
    }
    return _loadFirstPage(config, pubkey, generation);
  }

  Future<void> _replaceCache(
    RelayConfig config,
    String pubkey,
    int generation,
  ) async {
    try {
      final list = await _loadFirstPage(config, pubkey, generation);
      if (generation != _generation || !ref.mounted) return;
      state = AsyncData(list);
    } catch (error, stack) {
      if (generation != _generation || !ref.mounted) return;
      if (isChatStateDisabledError(error)) {
        state = AsyncError(error, stack);
      } else {
        // Keep the saved rows and try again shortly.
        debugPrint('[ChatList] first page failed: $error');
        _reloadTimer?.cancel();
        _reloadTimer = Timer(_firstPageRetryDelay, () {
          _reloadTimer = null;
          _reloadIfCurrent(generation);
        });
      }
    }
  }

  Future<ChatListState> _loadFirstPage(
    RelayConfig config,
    String pubkey,
    int generation,
  ) async {
    _loadedAt = DateTime.now();
    _backgrounded = false;
    final buffered = _eventsDuringLoad = [];
    final ChatListPage page;
    _firstPageInFlight = true;
    try {
      page = await _fetchPage(null);
    } finally {
      if (identical(_eventsDuringLoad, buffered)) _eventsDuringLoad = null;
      if (generation == _generation) {
        _firstPageInFlight = false;
        if (_pinsChangedDuringLoad) {
          _pinsChangedDuringLoad = false;
          Timer.run(() => _reloadSoon(generation, immediate: true));
        }
      }
    }
    var list = mergeRefetchedFirstPage(state.value, page);
    var reload = false;
    for (final event in buffered) {
      final result = _foldLiveEvent(list, event, replay: true);
      list = result.state;
      reload = reload || result.reload;
    }
    _writeCache(config, pubkey, page);
    if (reload) {
      Timer.run(() => _reloadSoon(generation));
    } else if (list.next != null) {
      Timer.run(() => unawaited(_loadMorePages(generation)));
    }
    return list;
  }

  Future<void> _subscribePing(String pubkey, int generation) async {
    try {
      final unsubscribe = await ref
          .read(relaySessionProvider.notifier)
          .subscribe(
            NostrFilter(
              kinds: const [chatStatePingKind],
              tags: {
                '#p': [pubkey.toLowerCase()],
              },
              limit: 0,
            ),
            (_) {
              if (ref.read(chatStateWriterProvider)?.isOwnWriteEcho() ??
                  false) {
                return;
              }
              _reloadSoon(generation, immediate: true);
            },
          );
      if (generation != _generation || !ref.mounted) {
        unsubscribe();
        return;
      }
      ref.onDispose(unsubscribe);
    } catch (error) {
      debugPrint('[ChatList] ping subscription failed: $error');
    }
  }

  Future<ChatListPage> _fetchPage(ChatListCursor? before) async {
    final response = await ref
        .read(relaySessionProvider.notifier)
        .queryRelayJson([
          buildChatListFilter(
            limit: chatListPageSize,
            before: before,
            pinnedThreads: ref.read(threadPinsProvider).toList()..sort(),
          ),
        ], timeout: chatListQueryTimeout);
    return parseChatListResponse(response);
  }

  Future<void> _loadMorePages(int generation) async {
    while (generation == _generation && ref.mounted) {
      final current = state.value;
      final next = current?.next;
      if (current == null ||
          next == null ||
          current.rows.length >= chatListBackgroundRows) {
        return;
      }
      final ChatListPage page;
      try {
        page = await _fetchPage(next);
      } catch (error) {
        debugPrint('[ChatList] page failed: $error');
        return;
      }
      if (generation != _generation || !ref.mounted) return;
      final latest = state.value;
      if (latest == null || latest.next != next) return;
      state = AsyncData(mergeChatListPage(latest, page));
    }
  }

  void _handleLiveEvent(NostrEvent event, int generation) {
    if (generation != _generation) return;
    final buffered = _eventsDuringLoad;
    if (buffered != null) {
      buffered.add(event);
      return;
    }
    final current = state.value;
    if (current == null || current.fromCache) return;
    final result = _foldLiveEvent(current, event);
    if (!identical(result.state, current)) state = AsyncData(result.state);
    if (result.reload) _reloadSoon(generation, immediate: true);
  }

  ({ChatListState state, bool reload}) _foldLiveEvent(
    ChatListState current,
    NostrEvent event, {
    bool replay = false,
  }) {
    final channelId = event.channelId;
    if (channelId == null) return (state: current, reload: false);
    if (event.kind == EventKind.deletion ||
        event.kind == EventKind.nip29DeleteEvent) {
      final targets = [
        for (final tag in event.tags)
          if (tag.length > 1 && tag[0] == 'e') tag[1],
      ];
      return (state: current, reload: chatListShowsAny(current, targets));
    }
    final result = applyLiveEventToChatList(
      current,
      event,
      channelId: channelId,
      currentPubkey: ref.read(myPubkeyProvider),
      replay: replay,
      readAtOf: (row) =>
          ref.read(readStateProvider).effectiveTimestamp(chatRowContextId(row)),
    );
    return (state: result.state, reload: result.refetch);
  }

  /// Reloads now, or once the minimum interval since the last load has
  /// passed. [immediate] skips the interval (rows the relay must place) and
  /// replaces a slower reload already waiting.
  void _reloadSoon(int generation, {bool immediate = false}) {
    if (generation != _generation) return;
    if (_reloadTimer != null) {
      if (!immediate) return;
      _reloadTimer!.cancel();
      _reloadTimer = null;
    }
    final wait = immediate
        ? const Duration(milliseconds: 300)
        : ref.read(chatListMinReloadIntervalProvider) -
              DateTime.now().difference(_loadedAt);
    if (wait <= Duration.zero) {
      _reloadIfCurrent(generation);
      return;
    }
    _reloadTimer = Timer(wait, () {
      _reloadTimer = null;
      _reloadIfCurrent(generation);
    });
  }

  void _reloadIfCurrent(int generation) {
    if (generation != _generation || !ref.mounted) return;
    if (ref.read(relaySessionProvider).status != SessionStatus.connected) {
      return;
    }
    ref.invalidateSelf();
  }

  /// Reloads the first page (pull to refresh).
  Future<void> refresh() async {
    ref.invalidateSelf();
    try {
      await future;
    } catch (error) {
      debugPrint('[ChatList] refresh failed: $error');
    }
  }

  ChatListState? _readCache(RelayConfig config, String pubkey) {
    final raw = ref
        .read(savedPrefsProvider)
        .getString(_cacheKey(config, pubkey));
    if (raw == null) return null;
    try {
      final page = parseChatListResponse(jsonDecode(raw));
      return ChatListState(rows: page.rows, next: null, fromCache: true);
    } catch (_) {
      return null;
    }
  }

  void _writeCache(RelayConfig config, String pubkey, ChatListPage page) {
    final json = jsonEncode({
      'chat_list': 1,
      'rows': [for (final row in page.rows) row.toJson()],
    });
    unawaited(
      ref.read(savedPrefsProvider).setString(_cacheKey(config, pubkey), json),
    );
  }
}

/// One retry for a failure that may pass, timeouts included (the relay or
/// the request running out of time); none when the relay keeps no chat
/// state.
Duration? chatListRetry(int retryCount, Object error) {
  if (retryCount > 0 || isChatStateDisabledError(error)) return null;
  return const Duration(seconds: 1);
}

/// The signed-in account's list owner, or null when signed out.
final chatListOwnerProvider = Provider<String?>((ref) {
  final config = ref.watch(relayConfigProvider);
  final pubkey = ref.watch(myPubkeyProvider);
  if (pubkey == null || pubkey.isEmpty) return null;
  return _ownerOf(config, pubkey);
});

final chatListForOwnerProvider = AsyncNotifierProvider.autoDispose
    .family<ChatListNotifier, ChatListState, String>(
      ChatListNotifier.new,
      retry: chatListRetry,
    );

/// The signed-in account's Chats list.
final chatListProvider = Provider<AsyncValue<ChatListState>>((ref) {
  final owner = ref.watch(chatListOwnerProvider);
  if (owner == null) {
    return const AsyncData(ChatListState(rows: [], next: null));
  }
  return ref.watch(chatListForOwnerProvider(owner));
});

/// Reloads the signed-in account's list (pull to refresh).
Future<void> refreshChatList(WidgetRef ref) async {
  final owner = ref.read(chatListOwnerProvider);
  if (owner == null) return;
  await ref.read(chatListForOwnerProvider(owner).notifier).refresh();
}

/// Whether the Chats screen must show the channel list instead: the relay
/// keeps no chat state, or the first page failed for good.
bool chatListFailed(AsyncValue<ChatListState> list) =>
    list.hasError && !list.hasValue && !list.retrying;
