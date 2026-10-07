import 'dart:async';
import 'dart:convert';
import 'dart:math' as math;

import 'package:flutter/foundation.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:shared_preferences/shared_preferences.dart';

import '../../shared/relay/relay.dart';
import '../../shared/theme/theme_provider.dart';

typedef ThreadNameKey = ({String channelId, String headId});

int threadNameWeight(String name) =>
    name.runes.fold(0, (weight, rune) => weight + (rune < 128 ? 1 : 2));

String? threadNameError(String name) {
  if (name.runes.any(
    (r) => r < 32 || (r >= 127 && r <= 159) || r == 0x2028 || r == 0x2029,
  )) {
    return 'Use a single line without control characters.';
  }
  if (threadNameWeight(name) > 40) {
    return 'Up to 40 English or 20 Korean characters.';
  }
  return null;
}

NostrEvent? latestThreadName(ThreadNameKey key, Iterable<NostrEvent> events) {
  NostrEvent? latest;
  for (final event in events) {
    final heads = event.tags
        .where((tag) => tag.isNotEmpty && tag[0] == 'e')
        .toList();
    final channels = event.tags
        .where((tag) => tag.isNotEmpty && tag[0] == 'h')
        .toList();
    if (event.kind != EventKind.threadName ||
        heads.length != 1 ||
        channels.length != 1 ||
        heads.single.length != 2 ||
        channels.single.length != 2 ||
        heads.single[1] != key.headId ||
        channels.single[1] != key.channelId ||
        event.content.trim() != event.content ||
        threadNameError(event.content) != null) {
      continue;
    }
    if (latest == null ||
        event.createdAt > latest.createdAt ||
        (event.createdAt == latest.createdAt &&
            event.id.compareTo(latest.id) < 0)) {
      latest = event;
    }
  }
  return latest;
}

/// Cached names and unsent editor drafts, isolated by community and identity.
/// Confirmed cache entries are bounded; drafts are never evicted with the cache.
class ThreadNameStorage {
  final SharedPreferences prefs;
  final String scope;
  ThreadNameStorage(this.prefs, this.scope);
  String _key(ThreadNameKey key) => '$scope:${key.channelId}:${key.headId}';
  NostrEvent? read(ThreadNameKey key) {
    final raw = prefs.getString('buzz-thread-name:${_key(key)}');
    if (raw == null) return null;
    try {
      return latestThreadName(key, [
        NostrEvent.fromJson(jsonDecode(raw) as Map<String, dynamic>),
      ]);
    } catch (error) {
      debugPrint('Ignoring invalid thread name cache: $error');
      return null;
    }
  }

  Future<void> write(ThreadNameKey key, NostrEvent event) async {
    final selected = latestThreadName(key, [?read(key), event]);
    if (selected == null) return;
    final cacheKey = 'buzz-thread-name:${_key(key)}';
    if (!await prefs.setString(cacheKey, jsonEncode(selected.toJson()))) {
      throw StateError('Could not cache the thread name.');
    }
    final keys =
        prefs
            .getKeys()
            .where((k) => k.startsWith('buzz-thread-name:$scope:'))
            .toList()
          ..sort();
    for (final old
        in keys
            .where((k) => k != cacheKey)
            .take(math.max(0, keys.length - 500))) {
      await prefs.remove(old);
    }
  }

  String? draft(ThreadNameKey key) =>
      prefs.getString('buzz-thread-name-draft:${_key(key)}');
  Future<void> saveDraft(ThreadNameKey key, String text) async {
    if (!await prefs.setString('buzz-thread-name-draft:${_key(key)}', text)) {
      throw StateError('Could not keep the draft on this device.');
    }
  }

  Future<void> clearDraft(ThreadNameKey key) async {
    if (!await prefs.remove('buzz-thread-name-draft:${_key(key)}')) {
      throw StateError('Could not clear the saved draft.');
    }
  }
}

final threadNameStorageProvider = Provider<ThreadNameStorage>((ref) {
  final config = ref.watch(relayConfigProvider);
  return ThreadNameStorage(
    ref.watch(savedPrefsProvider),
    '${config.baseUrl}:${outgoingAuthorPubkey(config) ?? "anonymous"}',
  );
});

/// Keeps the cached name visible when refreshing fails.
class ThreadNameState {
  final NostrEvent? value;
  final Object? error;
  const ThreadNameState(this.value, {this.error});
}

class ThreadNameNotifier extends Notifier<ThreadNameState> {
  final ThreadNameKey key;
  ThreadNameNotifier(this.key);
  int _generation = 0;
  bool _saving = false;
  NostrEvent? _latest;

  NostrFilter get _filter => NostrFilter(
    kinds: const [EventKind.threadName],
    tags: {
      '#h': [key.channelId],
      '#e': [key.headId],
    },
    limit: 1,
  );

  @override
  ThreadNameState build() {
    final storage = ref.watch(threadNameStorageProvider);
    final connected =
        ref.watch(relaySessionProvider.select((s) => s.status)) ==
        SessionStatus.connected;
    final generation = ++_generation;
    _latest = storage.read(key);
    ref.onDispose(() {
      _generation++;
    });
    if (connected) unawaited(_load(generation, storage));
    return ThreadNameState(_latest);
  }

  bool _current(int generation) => ref.mounted && generation == _generation;

  Future<void> _merge(
    int generation,
    ThreadNameStorage storage,
    Iterable<NostrEvent> events,
  ) async {
    if (!_current(generation)) return;
    final next = latestThreadName(key, [?_latest, ...events]);
    if (next == null) {
      state = ThreadNameState(_latest);
      return;
    }
    _latest = next;
    state = ThreadNameState(next);
    await storage.write(key, next);
  }

  void _failure(int generation, Object error, StackTrace stack) {
    if (_current(generation)) {
      state = ThreadNameState(_latest, error: error);
    }
  }

  Future<void> _load(int generation, ThreadNameStorage storage) async {
    final session = ref.read(relaySessionProvider.notifier);
    void Function()? stop;
    ref.onDispose(() => stop?.call());
    try {
      // Subscribe before history to cover edits arriving during the query.
      stop = await session.subscribe(
        _filter,
        (event) {
          unawaited(
            _merge(generation, storage, [event]).catchError(
              (Object e, StackTrace st) => _failure(generation, e, st),
            ),
          );
        },
        onClosed: (message) =>
            _failure(generation, StateError(message), StackTrace.current),
      );
      if (!_current(generation)) {
        stop();
        return;
      }
      await _merge(generation, storage, await session.queryRelay([_filter]));
    } catch (error, stack) {
      _failure(generation, error, stack);
    }
  }

  Future<void> save(String input) async {
    final name = input.trim();
    final error = threadNameError(name);
    if (error != null) throw FormatException(error);
    if (_saving) throw StateError('A name is already being saved.');
    final generation = _generation;
    final storage = ref.read(threadNameStorageProvider);
    final config = ref.read(relayConfigProvider);
    final session = ref.read(relaySessionProvider.notifier);
    _saving = true;
    try {
      await storage.saveDraft(key, input);
      final previous = latestThreadName(key, [
        ?_latest,
        ...await session.queryRelay([_filter]),
      ]);
      if (!_current(generation)) {
        throw StateError('Connection changed. Please retry.');
      }
      final event = buildOutgoingEvent(
        config,
        kind: EventKind.threadName,
        content: name,
        tags: [
          ['h', key.channelId],
          ['e', key.headId],
        ],
        createdAt: math.max(
          DateTime.now().millisecondsSinceEpoch ~/ 1000,
          (previous?.createdAt ?? 0) + 1,
        ),
      );
      final accepted = await session.publish(event);
      if (!_current(generation)) {
        throw StateError(
          'Connection changed. Reopen the thread to check the saved name.',
        );
      }
      // Query relay truth as token-mode submissions may be re-signed.
      final stored = await session.queryRelay([_filter]);
      if (!_current(generation)) {
        throw StateError(
          'Connection changed. Reopen the thread to check the saved name.',
        );
      }
      await _merge(generation, storage, stored.isEmpty ? [accepted] : stored);
      await storage.clearDraft(key);
    } finally {
      _saving = false;
    }
  }
}

final threadNameProvider = NotifierProvider.autoDispose
    .family<ThreadNameNotifier, ThreadNameState, ThreadNameKey>(
      ThreadNameNotifier.new,
    );
