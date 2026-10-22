import 'dart:async';
import 'dart:convert';
import 'dart:math';

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

/// Unpins kept as `false` registers so the unpin reaches other devices
/// (desktop `MAX_ID_SET_TOMBSTONES`).
const maxThreadPinTombstones = 50;

/// The relay refuses a record whose ciphertext is longer.
const _maxCiphertextBytes = 65535;

const _deviceIdKey = 'kuzz.threadPins.dev.v1';

final _threadIdPattern = RegExp(r'^[0-9a-f]{64}$');

/// Thread ids pinned in a desktop `thread-pins` record: the `true` registers
/// of `meta.i`. A record without `meta` reads as no pins, as desktop's
/// `decodeDoc`; anything that is not a 64-hex event id is skipped (the relay
/// refuses the whole list request for one bad id).
Set<String> parseThreadPins(Object? json) =>
    pinnedIdsOf(parseThreadPinRegisters(json));

/// One id's register in the record: `[version ms, device, pinned]`.
typedef ThreadPinRegister = ({int v, String dev, bool pinned});

/// The `meta.i` registers of a desktop `thread-pins` record, keyed by
/// lowercase thread id.
Map<String, ThreadPinRegister> parseThreadPinRegisters(Object? json) {
  if (json is! Map<String, dynamic> || json['version'] != 1) return const {};
  final meta = json['meta'];
  if (meta is! Map<String, dynamic> || meta['v'] != 1) return const {};
  final registers = meta['i'];
  if (registers is! Map<String, dynamic>) return const {};
  final result = <String, ThreadPinRegister>{};
  for (final entry in registers.entries) {
    final reg = entry.value;
    final id = entry.key.toLowerCase();
    if (reg is! List || reg.length != 3) continue;
    final (v, dev, pinned) = (reg[0], reg[1], reg[2]);
    if (v is! int || dev is! String || pinned is! bool) continue;
    if (!_threadIdPattern.hasMatch(id)) continue;
    final next = (v: v, dev: dev, pinned: pinned);
    final held = result[id];
    if (held == null || _compareRegisters(next, held) > 0) result[id] = next;
  }
  return result;
}

/// Ids whose register is `true`, at most [maxThreadPins].
Set<String> pinnedIdsOf(Map<String, ThreadPinRegister> registers) => {
  for (final entry in registers.entries)
    if (entry.value.pinned) entry.key,
}.take(maxThreadPins).toSet();

/// Desktop `compareRegs`: version, then device, then unpin over pin.
int _compareRegisters(ThreadPinRegister a, ThreadPinRegister b) {
  if (a.v != b.v) return a.v.compareTo(b.v);
  if (a.dev != b.dev) return a.dev.compareTo(b.dev);
  if (a.pinned == b.pinned) return 0;
  return a.pinned ? -1 : 1;
}

/// Merges [incoming] into [held], keeping the higher register per id.
Map<String, ThreadPinRegister> mergeThreadPinRegisters(
  Map<String, ThreadPinRegister> held,
  Map<String, ThreadPinRegister> incoming,
) {
  final result = Map.of(held);
  for (final entry in incoming.entries) {
    final current = result[entry.key];
    if (current == null || _compareRegisters(entry.value, current) > 0) {
      result[entry.key] = entry.value;
    }
  }
  return result;
}

/// Sets one id's register above every version seen, then keeps only the
/// newest [maxThreadPinTombstones] unpins. Unpinning an id with no register
/// changes nothing.
Map<String, ThreadPinRegister> setThreadPinRegister(
  Map<String, ThreadPinRegister> registers,
  String id,
  bool pinned, {
  required int nowMs,
  required String dev,
}) {
  final key = id.toLowerCase();
  final held = registers[key];
  if (!pinned && held == null) return registers;
  final seen = registers.values.fold<int>(0, (top, reg) => max(top, reg.v));
  final result = Map.of(registers)
    ..[key] = (v: max(nowMs, seen + 1), dev: dev, pinned: pinned);
  final tombstones =
      result.entries.where((entry) => !entry.value.pinned).toList()
        ..sort((a, b) => b.value.v.compareTo(a.value.v));
  for (final stale in tombstones.skip(maxThreadPinTombstones)) {
    result.remove(stale.key);
  }
  return result;
}

/// The record desktop writes: `{version, ids, meta: {v, i}}`.
Map<String, dynamic> encodeThreadPins(
  Map<String, ThreadPinRegister> registers,
) => {
  'version': 1,
  'ids': pinnedIdsOf(registers).toList()..sort(),
  'meta': {
    'v': 1,
    'i': {
      for (final entry in registers.entries)
        entry.key: [entry.value.v, entry.value.dev, entry.value.pinned],
    },
  },
};

String _cacheKey(String origin, String pubkey) =>
    'kuzz.threadPins.v1:${storageScopeFor(origin)}:${pubkey.toLowerCase()}';

/// Threads the user pinned, so the Chats list asks the relay to keep them
/// listed while quiet or closed, as desktop does. Saved on this device so the
/// first `chat_list` request after a start already has them. Pins sync
/// through the same record as desktop; a token community has no key for it,
/// so its pins stay on this device.
class ThreadPinsNotifier extends Notifier<Set<String>> {
  int _generation = 0;

  /// `created_at` of the record the pins came from; an older one is ignored.
  int _appliedAt = 0;

  /// Registers of the newest record seen, merged with this device's writes.
  Map<String, ThreadPinRegister> _registers = const {};
  String? _nsec;
  String? _pubkey;
  String? _key;

  @override
  Set<String> build() {
    final config = ref.watch(relayConfigProvider);
    final connected = ref.watch(
      relaySessionProvider.select((s) => s.status == SessionStatus.connected),
    );
    final generation = ++_generation;
    _appliedAt = 0;
    _registers = const {};
    _nsec = null;
    _pubkey = null;
    _key = null;
    ref.onDispose(() => _generation++);
    final trimmed = config.tokenAuth ? null : config.nsec?.trim();
    final nsec = trimmed == null || trimmed.isEmpty ? null : trimmed;
    final pubkey = nsec == null
        ? (config.tokenAuth ? config.principalId : null)
        : ref.watch(myPubkeyProvider);
    if (pubkey == null || pubkey.isEmpty) return const {};
    final key = _cacheKey(config.baseUrl, pubkey);
    _key = key;
    _pubkey = pubkey.toLowerCase();
    _nsec = nsec;
    final saved = ref
        .read(savedPrefsProvider)
        .getStringList(key)
        ?.where((id) => _threadIdPattern.hasMatch(id));
    if (connected && nsec != null) {
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

  /// True once the newest record (or that there is none) has been read.
  Future<bool> _load(
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
      if (generation != _generation || !ref.mounted) return false;
      final event = events
          .where((event) => event.pubkey.toLowerCase() == pubkey)
          .fold<NostrEvent?>(
            null,
            (best, event) =>
                best == null || event.createdAt > best.createdAt ? event : best,
          );
      if (event == null) {
        if (_appliedAt == 0 && _registers.isEmpty) _set(const {}, key);
        return true;
      }
      _apply(event, nsec, pubkey, key);
      return true;
    } catch (error) {
      // Keep the saved pins; the next start or resume reads again.
      debugPrint('[ThreadPins] read failed: $error');
      return false;
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
    final registers = parseThreadPinRegisters(
      jsonDecode(ChannelStarsCrypto(nsec, pubkey).decrypt(event.content)),
    );
    _appliedAt = event.createdAt;
    _registers = mergeThreadPinRegisters(_registers, registers);
    _set(pinnedIdsOf(_registers), key);
  }

  /// Pins or unpins [rootId]. False (nothing changed) when [pinned] would go
  /// past [maxThreadPins].
  bool setPinned(String rootId, bool pinned) {
    final key = _key;
    if (key == null) return false;
    final id = rootId.toLowerCase();
    if (pinned && !state.contains(id) && state.length >= maxThreadPins) {
      return false;
    }
    final nsec = _nsec;
    final pubkey = _pubkey;
    if (nsec == null || pubkey == null) {
      _set(pinned ? {...state, id} : ({...state}..remove(id)), key);
      return true;
    }
    _registers = setThreadPinRegister(
      _registers,
      id,
      pinned,
      nowMs: DateTime.now().millisecondsSinceEpoch,
      dev: _deviceId(),
    );
    _set(pinnedIdsOf(_registers), key);
    unawaited(_publish(nsec, pubkey, key, _generation));
    return true;
  }

  String _deviceId() {
    final prefs = ref.read(savedPrefsProvider);
    final saved = prefs.getString(_deviceIdKey);
    if (saved != null && RegExp(r'^[0-9a-f]{16}$').hasMatch(saved)) {
      return saved;
    }
    final random = Random.secure();
    final dev = List.generate(
      8,
      (_) => random.nextInt(256).toRadixString(16).padLeft(2, '0'),
    ).join();
    unawaited(prefs.setString(_deviceIdKey, dev));
    return dev;
  }

  /// Read-before-write, as desktop: merge the newest record, then publish.
  Future<void> _publish(
    String nsec,
    String pubkey,
    String key,
    int generation,
  ) async {
    try {
      // Publishing without the newest record would drop other devices' pins.
      if (!await _load(nsec, pubkey, key, generation)) return;
      if (generation != _generation || !ref.mounted) return;
      final crypto = ChannelStarsCrypto(nsec, pubkey);
      final content = crypto.encrypt(jsonEncode(encodeThreadPins(_registers)));
      if (utf8.encode(content).length > _maxCiphertextBytes) return;
      final createdAt = max(
        DateTime.now().millisecondsSinceEpoch ~/ 1000,
        _appliedAt + 1,
      );
      await SignedEventRelay(
        session: ref.read(relaySessionProvider.notifier),
        nsec: nsec,
      ).submit(
        kind: EventKind.readState,
        content: content,
        tags: const [
          ['d', threadPinsDTag],
          ['t', threadPinsDTag],
        ],
        createdAt: createdAt,
      );
      if (generation == _generation && createdAt > _appliedAt) {
        _appliedAt = createdAt;
      }
    } catch (error) {
      debugPrint('[ThreadPins] publish failed: $error');
    }
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
