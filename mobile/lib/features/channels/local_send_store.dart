import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:shared_preferences/shared_preferences.dart';

import '../../shared/relay/relay.dart';
import '../../shared/theme/theme_provider.dart';
import 'channel_messages_provider.dart';
import 'failed_local_messages_provider.dart';
import 'pending_local_messages_provider.dart';

const _localSendsPrefsKey = 'local_sends_v1';
const _maxStoredSends = 50;
const _maxStoredAge = Duration(days: 7);
const _restoreLookupTimeout = Duration(seconds: 10);

/// Device-local copy of own composer sends the relay has not confirmed, so a
/// failed (or still sending) message survives an app restart like a composer
/// draft did before sends showed instantly.
///
/// Entries hold the full signed event, which also carries its thread
/// placement, under one key per community, signer and channel: another
/// account or community never reads them. Each key keeps the newest
/// [_maxStoredSends] sends of the last [_maxStoredAge].
class LocalSendStore {
  final SharedPreferences _prefs;
  final String _scope;

  LocalSendStore(this._prefs, this._scope);

  String keyFor(String signer, String channelId) =>
      '$_localSendsPrefsKey:$_scope:${signer.toLowerCase()}:$channelId';

  List<NostrEvent> read(String key) {
    final raw = _prefs.getString(key);
    if (raw == null) return const [];
    final List<NostrEvent> stored;
    try {
      stored = [
        for (final entry in jsonDecode(raw) as List<dynamic>)
          NostrEvent.fromJson(entry as Map<String, dynamic>),
      ];
    } catch (_) {
      return const [];
    }
    final kept = _bounded(stored);
    if (kept.length != stored.length) _write(key, kept);
    return kept;
  }

  void put(String key, NostrEvent event) => _write(
    key,
    _bounded([...read(key).where((stored) => stored.id != event.id), event]),
  );

  void remove(String key, Set<String> eventIds) {
    final stored = read(key);
    final next = stored.where((event) => !eventIds.contains(event.id));
    if (next.length != stored.length) _write(key, next.toList());
  }

  static List<NostrEvent> _bounded(List<NostrEvent> events) {
    final oldest =
        DateTime.now().millisecondsSinceEpoch ~/ 1000 - _maxStoredAge.inSeconds;
    final kept = events.where((event) => event.createdAt >= oldest).toList()
      ..sort((a, b) => a.createdAt.compareTo(b.createdAt));
    return kept.length > _maxStoredSends
        ? kept.sublist(kept.length - _maxStoredSends)
        : kept;
  }

  void _write(String key, List<NostrEvent> events) {
    if (events.isEmpty) {
      _prefs.remove(key);
      return;
    }
    _prefs.setString(
      key,
      jsonEncode([for (final event in events) event.toJson()]),
    );
  }
}

/// The store for the active community, or null where no preferences are
/// available (some tests), which leaves local sends in memory only.
final localSendStoreProvider = Provider<LocalSendStore?>((ref) {
  final config = ref.watch(relayConfigProvider);
  try {
    return LocalSendStore(
      ref.watch(savedPrefsProvider),
      storageScopeFor(config.baseUrl),
    );
  } catch (_) {
    return null;
  }
});

/// Shows [signer]'s sends in [messages]' channel that an earlier run never
/// got confirmed, at their original place.
///
/// The app may have closed after the relay stored a send but before it said
/// so, so the rows wait as pending for one lookup by id: the relay's copy
/// confirms a row, and a row it lacks, or a lookup that fails, is marked
/// failed for retry or delete.
Future<void> restoreLocalSends(
  Ref ref,
  ChannelMessagesNotifier messages,
  String? signer,
) async {
  if (!ref.mounted || signer == null) return;
  final channelId = messages.channelId;
  final stored = ref
      .read(pendingLocalMessagesProvider(channelId).notifier)
      .storedSends(signer);
  if (stored.isEmpty) return;
  for (final event in stored) {
    messages.addLocalMessage(event, persist: true);
  }
  final ids = [for (final event in stored) event.id];
  var delivered = const <String>{};
  try {
    final found = await ref
        .read(relaySessionProvider.notifier)
        .queryRelay([
          NostrFilter(
            ids: ids,
            kinds: {for (final event in stored) event.kind}.toList(),
            tags: {
              '#h': [channelId],
            },
            limit: ids.length,
          ),
        ], timeout: _restoreLookupTimeout)
        .timeout(_restoreLookupTimeout);
    delivered = {for (final event in found) event.id};
  } catch (error) {
    debugPrint('[restoreLocalSends] lookup failed for $channelId: $error');
  }
  if (!ref.mounted) return;
  final failed = ref.read(failedLocalMessagesProvider(channelId).notifier);
  for (final id in ids) {
    if (delivered.contains(id)) {
      messages.completeLocalMessage(id);
    } else {
      failed.markFailed(id);
    }
  }
}
