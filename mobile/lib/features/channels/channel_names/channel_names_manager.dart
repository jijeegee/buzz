import 'dart:async';
import 'dart:convert';
import 'dart:math';

import 'package:flutter/foundation.dart';
import 'package:shared_preferences/shared_preferences.dart';

import '../../../shared/read_state/read_state_time.dart';
import '../../../shared/relay/relay.dart';
import '../channel_mutes/channel_mutes_manager.dart' show ChannelMutesCrypto;
import 'channel_names_storage.dart';

/// Private chat names ("Rename for me"): kept on this device and synced to
/// my other devices as one kind 30078 `d=channel-names` document, NIP-44
/// encrypted to myself, last write wins per chat. Same sync shape as
/// `channel-mutes`; nobody else ever sees these names.
class ChannelNamesManager {
  ChannelNamesManager({
    required this.pubkey,
    required SharedPreferences prefs,
    required ChannelMutesCrypto? crypto,
    required RelaySessionNotifier? relaySession,
    required SignedEventRelay? signedEventRelay,
    required bool remoteEnabled,
    required VoidCallback onChanged,
    @visibleForTesting Duration publishDelay = const Duration(seconds: 2),
  }) : _storage = ChannelNamesStorage(prefs),
       _crypto = crypto,
       _relaySession = relaySession,
       _signedEventRelay = signedEventRelay,
       _remoteEnabled = remoteEnabled && crypto != null,
       _onChanged = onChanged,
       _publishDelay = publishDelay,
       _store = ChannelNamesStorage(prefs).read(pubkey);

  final String pubkey;
  final ChannelNamesStorage _storage;
  final ChannelMutesCrypto? _crypto;
  final RelaySessionNotifier? _relaySession;
  final SignedEventRelay? _signedEventRelay;
  final bool _remoteEnabled;
  final VoidCallback _onChanged;
  final Duration _publishDelay;

  ChannelNameStore _store;
  Timer? _publishTimer;
  int _lastRemoteCreatedAt = 0;
  String? _lastRemoteEventId;
  void Function()? _unsubscribe;
  bool _disposed = false;

  ChannelNameStore get store => _store;

  Future<void> initialize() async {
    if (_disposed || !_remoteEnabled || _relaySession == null) {
      _onChanged();
      return;
    }
    await refreshFromRelay();
    if (_disposed) return;
    try {
      final unsubscribe = await _relaySession.subscribe(
        _filter(),
        (event) => _applyEvents([event]),
        onClosed: (message) =>
            debugPrint('[ChannelNamesManager] live closed: $message'),
      );
      if (_disposed) {
        unsubscribe();
      } else {
        _unsubscribe = unsubscribe;
      }
    } catch (error) {
      debugPrint('[ChannelNamesManager] live subscription failed: $error');
    }
  }

  /// Re-reads my latest document, e.g. when the app returns to the
  /// foreground. Skipped while a local change waits to be published.
  Future<void> refreshFromRelay() async {
    if (_disposed || !_remoteEnabled || _publishTimer != null) return;
    final events = await _fetchHead();
    if (events != null && !_disposed && _publishTimer == null) {
      _applyEvents(events);
    }
  }

  /// Sets my private name for [channelId]; a blank [name] restores it.
  void setName(String channelId, String? name) {
    if (_disposed) return;
    final normalized = normalizeChannelName(name);
    if (_store.nameFor(channelId) == normalized) return;
    _store = ChannelNameStore(
      channels: {
        ..._store.channels,
        channelId: ChannelNameEntry(
          name: normalized,
          updatedAt: max(currentUnixSeconds(), _lastRemoteCreatedAt + 1),
        ),
      },
    );
    unawaited(_persist());
    _onChanged();
    if (!_remoteEnabled) return;
    _publishTimer?.cancel();
    _publishTimer = Timer(_publishDelay, () => unawaited(_publish()));
  }

  void dispose() {
    if (_disposed) return;
    _disposed = true;
    final pending = _publishTimer != null;
    _publishTimer?.cancel();
    _publishTimer = null;
    if (pending && _remoteEnabled) unawaited(_publish(allowDisposed: true));
    _unsubscribe?.call();
    _unsubscribe = null;
  }

  NostrFilter _filter() => NostrFilter(
    kinds: const [EventKind.readState],
    authors: [pubkey],
    tags: const {
      '#d': [channelNamesDTag],
    },
    limit: 1,
  );

  Future<List<NostrEvent>?> _fetchHead() async {
    try {
      return await _relaySession!.fetchHistory(_filter());
    } catch (error) {
      debugPrint('[ChannelNamesManager] fetch failed: $error');
      return null;
    }
  }

  bool _isAfterCursor(int createdAt, String id) =>
      createdAt > _lastRemoteCreatedAt ||
      (createdAt == _lastRemoteCreatedAt &&
          id.compareTo(_lastRemoteEventId ?? '') < 0);

  void _applyEvents(List<NostrEvent> events) {
    if (_disposed) return;
    var changed = false;
    for (final event in events) {
      if (event.pubkey != pubkey ||
          event.getTagValue('d') != channelNamesDTag ||
          !_isAfterCursor(event.createdAt, event.id)) {
        continue;
      }
      final Map<String, String>? remote;
      try {
        remote = parseRemoteChannelNames(_crypto!.decrypt(event.content));
      } catch (error) {
        debugPrint('[ChannelNamesManager] undecodable head: $error');
        continue;
      }
      if (remote == null) continue;
      _store = mergeRemoteChannelNames(_store, remote, event.createdAt);
      _lastRemoteCreatedAt = event.createdAt;
      _lastRemoteEventId = event.id;
      changed = true;
    }
    if (!changed) return;
    unawaited(_persist());
    _onChanged();
  }

  Future<void> _publish({bool allowDisposed = false}) async {
    _publishTimer = null;
    if ((!allowDisposed && _disposed) || _signedEventRelay == null) return;
    // Read before write, so another device's newer names are kept.
    final events = await _fetchHead();
    if (events != null) _applyEvents(events);
    try {
      final createdAt = max(currentUnixSeconds(), _lastRemoteCreatedAt + 1);
      String? signedId;
      await _signedEventRelay.submit(
        kind: EventKind.readState,
        content: _crypto!.encrypt(jsonEncode(_store.toRemote())),
        tags: const [
          ['d', channelNamesDTag],
          ['t', channelNamesDTag],
        ],
        createdAt: createdAt,
        onSigned: (event) => signedId = event.id,
      );
      if (signedId case final id? when _isAfterCursor(createdAt, id)) {
        _lastRemoteCreatedAt = createdAt;
        _lastRemoteEventId = id;
      }
    } catch (error) {
      debugPrint('[ChannelNamesManager] publish failed: $error');
    }
  }

  Future<void> _persist() async {
    try {
      await _storage.write(pubkey, _store);
    } catch (error) {
      debugPrint('[ChannelNamesManager] persist failed: $error');
    }
  }
}
