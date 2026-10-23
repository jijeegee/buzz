import 'dart:convert';

import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:shared_preferences/shared_preferences.dart';

import '../../shared/relay/relay.dart';
import '../../shared/theme/theme_provider.dart';
import 'channel_messages_provider.dart';
import 'failed_local_messages_provider.dart';
import 'pending_local_messages_provider.dart';

const _localSendsPrefsKey = 'local_sends_v1';

/// Device-local copy of own sends the relay has not confirmed, so a failed
/// (or still sending) message survives an app restart like a composer draft
/// did before sends showed instantly.
///
/// Entries hold the full signed event, which also carries its thread
/// placement, under one key per community, signer and channel: another
/// account or community never reads them.
class LocalSendStore {
  final SharedPreferences _prefs;
  final String _scope;

  LocalSendStore(this._prefs, this._scope);

  String keyFor(String signer, String channelId) =>
      '$_localSendsPrefsKey:$_scope:${signer.toLowerCase()}:$channelId';

  List<NostrEvent> read(String key) {
    final raw = _prefs.getString(key);
    if (raw == null) return const [];
    try {
      return [
        for (final entry in jsonDecode(raw) as List<dynamic>)
          NostrEvent.fromJson(entry as Map<String, dynamic>),
      ];
    } catch (_) {
      return const [];
    }
  }

  void put(String key, NostrEvent event) => _write(key, [
    ...read(key).where((stored) => stored.id != event.id),
    event,
  ]);

  void remove(String key, Set<String> eventIds) {
    final stored = read(key);
    final next = stored.where((event) => !eventIds.contains(event.id));
    if (next.length != stored.length) _write(key, next.toList());
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
/// got confirmed, as failed rows at their original place. A send cut off by
/// the app closing is not known to have landed, so it is offered for retry
/// too; a relay copy confirms it like any echo.
void restoreLocalSends(
  Ref ref,
  ChannelMessagesNotifier messages,
  String? signer,
) {
  if (!ref.mounted || signer == null) return;
  final channelId = messages.channelId;
  final stored = ref
      .read(pendingLocalMessagesProvider(channelId).notifier)
      .storedSends(signer);
  final failed = ref.read(failedLocalMessagesProvider(channelId).notifier);
  for (final event in stored) {
    messages.addLocalMessage(event);
    failed.markFailed(event.id);
  }
}
