import 'dart:async';
import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/relay/relay.dart';
import '../../shared/theme/theme_provider.dart';
import '../channels/channel_stars/channel_stars_manager.dart';

/// The synced record desktop keeps thread pins in (encrypted to self).
const threadPinsDTag = 'thread-pins';

/// The relay lists at most this many pinned threads.
const maxThreadPins = 200;

final _threadIdPattern = RegExp(r'^[0-9a-f]{64}$');

/// Thread ids pinned in a desktop `thread-pins` record: the `true` registers
/// of `meta.i`. A record without `meta` reads as no pins, as desktop's
/// `decodeDoc`; anything that is not a 64-hex event id is skipped (the relay
/// refuses the whole list request for one bad id). Read only: phones cannot
/// pin threads yet.
Set<String> parseThreadPins(Object? json) {
  if (json is! Map<String, dynamic> || json['version'] != 1) return const {};
  final meta = json['meta'];
  if (meta is! Map<String, dynamic> || meta['v'] != 1) return const {};
  final registers = meta['i'];
  if (registers is! Map<String, dynamic>) return const {};
  return {
    for (final entry in registers.entries)
      if (entry.value is List &&
          (entry.value as List).length == 3 &&
          (entry.value as List)[2] == true &&
          _threadIdPattern.hasMatch(entry.key.toLowerCase()))
        entry.key.toLowerCase(),
  }.take(maxThreadPins).toSet();
}

String _cacheKey(String origin, String pubkey) =>
    'kuzz.threadPins.v1:$origin:${pubkey.toLowerCase()}';

/// Threads the user pinned (on desktop), so the Chats list asks the relay to
/// keep them listed while quiet or closed, as desktop does. Saved on this
/// device so the first `chat_list` request after a start already has them.
/// Token communities have no key to read the record and send none.
class ThreadPinsNotifier extends Notifier<Set<String>> {
  int _generation = 0;

  /// `created_at` of the record the pins came from; an older one is ignored.
  int _appliedAt = 0;

  @override
  Set<String> build() {
    final config = ref.watch(relayConfigProvider);
    final connected = ref.watch(
      relaySessionProvider.select((s) => s.status == SessionStatus.connected),
    );
    final generation = ++_generation;
    _appliedAt = 0;
    ref.onDispose(() => _generation++);
    final nsec = config.tokenAuth ? null : config.nsec?.trim();
    final pubkey = ref.watch(myPubkeyProvider);
    if (nsec == null || nsec.isEmpty || pubkey == null || pubkey.isEmpty) {
      return const {};
    }
    final key = _cacheKey(config.baseUrl, pubkey);
    final saved = ref
        .read(savedPrefsProvider)
        .getStringList(key)
        ?.where((id) => _threadIdPattern.hasMatch(id));
    if (connected) {
      void load() =>
          unawaited(_load(nsec, pubkey.toLowerCase(), key, generation));
      ref.listen(appLifecycleProvider, (_, next) {
        if (next == AppLifecycleState.resumed) load();
      });
      Timer.run(load);
      // Live changes from other devices, as desktop's pin lane.
      unawaited(_subscribe(nsec, pubkey.toLowerCase(), key, generation));
    }
    return saved == null ? const {} : saved.toSet();
  }

  Future<void> _load(
    String nsec,
    String pubkey,
    String key,
    int generation,
  ) async {
    try {
      final events = await ref.read(relaySessionProvider.notifier).queryRelay([
        NostrFilter(
          kinds: const [EventKind.readState],
          authors: [pubkey],
          tags: const {
            '#d': [threadPinsDTag],
          },
          limit: 1,
        ),
      ]);
      if (generation != _generation || !ref.mounted) return;
      final event = events
          .where((event) => event.pubkey.toLowerCase() == pubkey)
          .fold<NostrEvent?>(
            null,
            (best, event) =>
                best == null || event.createdAt > best.createdAt ? event : best,
          );
      if (event == null) {
        if (_appliedAt == 0) _set(const {}, key);
        return;
      }
      _apply(event, nsec, pubkey, key);
    } catch (error) {
      // Keep the saved pins; the next start or resume reads again.
      debugPrint('[ThreadPins] read failed: $error');
    }
  }

  Future<void> _subscribe(
    String nsec,
    String pubkey,
    String key,
    int generation,
  ) async {
    try {
      final unsubscribe = await ref
          .read(relaySessionProvider.notifier)
          .subscribe(
            NostrFilter(
              kinds: const [EventKind.readState],
              authors: [pubkey],
              tags: const {
                '#d': [threadPinsDTag],
              },
              limit: 0,
            ),
            (event) {
              if (generation != _generation || !ref.mounted) return;
              if (event.pubkey.toLowerCase() != pubkey) return;
              try {
                _apply(event, nsec, pubkey, key);
              } catch (error) {
                debugPrint('[ThreadPins] live record unreadable: $error');
              }
            },
          );
      if (generation != _generation || !ref.mounted) {
        unsubscribe();
        return;
      }
      ref.onDispose(unsubscribe);
    } catch (error) {
      debugPrint('[ThreadPins] subscribe failed: $error');
    }
  }

  void _apply(NostrEvent event, String nsec, String pubkey, String key) {
    if (event.createdAt < _appliedAt) return;
    final pins = parseThreadPins(
      jsonDecode(ChannelStarsCrypto(nsec, pubkey).decrypt(event.content)),
    );
    _appliedAt = event.createdAt;
    _set(pins, key);
  }

  void _set(Set<String> pins, String key) {
    if (setEquals(pins, state)) return;
    state = pins;
    unawaited(
      ref.read(savedPrefsProvider).setStringList(key, pins.toList()..sort()),
    );
  }
}

final threadPinsProvider = NotifierProvider<ThreadPinsNotifier, Set<String>>(
  ThreadPinsNotifier.new,
);
