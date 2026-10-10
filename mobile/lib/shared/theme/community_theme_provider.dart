import 'dart:async';

import 'package:flutter/material.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:nostr/nostr.dart' as nostr;

import '../crypto/nip44.dart';
import '../relay/relay.dart';
import 'accent_colors.dart';
import 'community_theme_preference.dart';
import 'community_theme_sync.dart';
import 'theme_provider.dart';

class CommunityThemeNotifier extends Notifier<CommunityThemePreference> {
  CommunityThemeSyncManager? _manager;
  late CommunityThemeStorage _storage;
  String? _pubkey;
  String? _relayUrl;
  Future<void> _persistenceQueue = Future<void>.value();
  int _localRevision = 0;
  CommunityThemePreference? _scopedLocalPreference;
  String? _scopedLocalPubkey;
  String? _scopedLocalRelayUrl;

  @override
  CommunityThemePreference build() {
    _manager?.dispose();
    _manager = null;

    _storage = ref.watch(communityThemeStorageProvider);
    final config = ref.watch(relayConfigProvider);
    final session = ref.watch(relaySessionProvider);
    final pubkey = pubkeyFromNsec(config.nsec);
    _pubkey = pubkey;
    _relayUrl = config.baseUrl;

    if (pubkey == null || config.nsec == null) {
      final legacy = _storage.legacyPreference();
      unawaited(_storage.writeLegacy(legacy));
      return legacy;
    }

    final cached = _storage.read(pubkey, config.baseUrl);
    final dirty = _storage.readOutbox(pubkey, config.baseUrl);
    final inMemoryLocal =
        _scopedLocalPubkey == pubkey && _scopedLocalRelayUrl == config.baseUrl
        ? _scopedLocalPreference
        : null;
    if (inMemoryLocal == null) {
      _scopedLocalPreference = null;
      _scopedLocalPubkey = pubkey;
      _scopedLocalRelayUrl = config.baseUrl;
    }
    final fallback = _storage.hasMigrated(pubkey)
        ? defaultCommunityTheme
        : _storage.legacyPreference();
    final synced = inMemoryLocal ?? dirty ?? cached ?? fallback;
    // Light/dark/system belongs to this device. Until it is stored, the
    // appearance this device last showed is its mode.
    final deviceMode = _storage.readDeviceMode();
    if (deviceMode == null) unawaited(_storage.writeDeviceMode(synced.mode));
    final initial = synced.withDeviceMode(deviceMode ?? synced.mode);

    if (session.status == SessionStatus.connected) {
      late final CommunityThemeSyncManager manager;
      manager = CommunityThemeSyncManager(
        pubkey: pubkey,
        relaySession: ref.read(relaySessionProvider.notifier),
        signedEventRelay: SignedEventRelay(
          session: ref.read(relaySessionProvider.notifier),
          nsec: config.nsec!,
        ),
        crypto: _crypto(config.nsec!, pubkey),
        onRemote: (remote) => _applyRemote(manager, remote),
        onPublished: (preference) {
          if (_scopedLocalPubkey == pubkey &&
              _scopedLocalRelayUrl == config.baseUrl &&
              _scopedLocalPreference == preference) {
            _scopedLocalPreference = null;
          }
          unawaited(_storage.clearOutbox(pubkey, config.baseUrl, preference));
        },
      );
      _manager = manager;
      final pending = inMemoryLocal ?? dirty;
      if (pending != null) manager.publish(pending);
      Future.microtask(() async {
        final result = await manager.initialize();
        if (_manager != manager) return;
        if (result.status == CommunityThemeRemoteStatus.absent) {
          final seedRevision = _localRevision;
          await _enqueuePersistence(() async {
            if (_manager != manager || _localRevision != seedRevision) return;
            final currentDirty = _storage.readOutbox(pubkey, config.baseUrl);
            final seed =
                currentDirty ?? _storage.read(pubkey, config.baseUrl) ?? state;
            if (!await _storage.write(pubkey, config.baseUrl, seed)) return;
            if (_manager != manager || _localRevision != seedRevision) return;
            if (!await _storage.writeOutbox(pubkey, config.baseUrl, seed)) {
              return;
            }
            if (_manager == manager && _localRevision == seedRevision) {
              manager.publish(seed);
            }
          });
        }
        if (result.status == CommunityThemeRemoteStatus.valid ||
            result.status == CommunityThemeRemoteStatus.absent) {
          await _storage.markMigrated(pubkey);
        }
      });
      ref.onDispose(manager.dispose);
    }
    return initial;
  }

  void setMode(ThemeMode mode) {
    var theme = state.theme;
    if (mode == ThemeMode.system) {
      theme = schemeForAppearanceMode(theme, mode) ?? theme;
    } else {
      final effective = effectiveTheme(theme, mode);
      if (effective != null) theme = effective.name;
    }
    _save(
      CommunityThemePreference(
        theme: theme,
        accent: state.accent,
        followSystem: mode == ThemeMode.system,
      ),
    );
  }

  void setTheme(String? theme) {
    _save(
      CommunityThemePreference(
        theme: theme ?? defaultSchemeName,
        accent: state.accent,
        followSystem: state.followSystem,
      ),
    );
  }

  void setAccent(int index) {
    if (index < 0 || index >= accentColors.length) return;
    _save(
      CommunityThemePreference(
        theme: state.theme,
        accent: accentColors[index].wireValue,
        followSystem: state.followSystem,
      ),
    );
  }

  /// Persists a complete theme-picker selection as one durable snapshot.
  void setPreference(CommunityThemePreference preference) {
    _save(preference);
  }

  void _save(CommunityThemePreference preference) {
    if (preference == state) return;
    final modeOnly = preference.sameSyncedChoice(state);
    state = preference;
    unawaited(_storage.writeDeviceMode(preference.mode));
    final pubkey = _pubkey;
    final relayUrl = _relayUrl;
    if (pubkey == null || relayUrl == null) {
      _localRevision++;
      unawaited(_storage.writeLegacy(preference));
      return;
    }
    // Light/dark/system is per device: keep it local and publish nothing.
    if (modeOnly) {
      unawaited(_storage.write(pubkey, relayUrl, preference));
      return;
    }
    _localRevision++;
    _scopedLocalPreference = preference;
    _scopedLocalPubkey = pubkey;
    _scopedLocalRelayUrl = relayUrl;
    _manager?.stage(preference);
    unawaited(
      _enqueuePersistence(
        () => _persistAndPublish(pubkey, relayUrl, preference),
      ),
    );
  }

  Future<void> _enqueuePersistence(Future<void> Function() operation) {
    final result = _persistenceQueue.then((_) => operation());
    _persistenceQueue = result.catchError((Object _) {});
    return result;
  }

  Future<void> _persistAndPublish(
    String pubkey,
    String relayUrl,
    CommunityThemePreference preference,
  ) async {
    if (!await _storage.write(pubkey, relayUrl, preference)) return;
    if (!await _storage.writeOutbox(pubkey, relayUrl, preference)) return;
    if (_pubkey == pubkey && _relayUrl == relayUrl) {
      final manager = _manager;
      if (manager != null) {
        manager.stage(preference);
        manager.publishStaged(preference);
      }
    }
  }

  void _applyRemote(
    CommunityThemeSyncManager manager,
    RemoteCommunityTheme remote,
  ) {
    if (_manager != manager) return;
    final pubkey = _pubkey;
    final relayUrl = _relayUrl;
    if (pubkey != null &&
        relayUrl != null &&
        _storage.readOutbox(pubkey, relayUrl) != null) {
      final dirty = _storage.readOutbox(pubkey, relayUrl)!;
      manager.publish(dirty);
      return;
    }
    state = remote.preference.withDeviceMode(
      _storage.readDeviceMode() ?? state.mode,
    );
    if (pubkey != null && relayUrl != null) {
      unawaited(_storage.write(pubkey, relayUrl, remote.preference));
    }
  }
}

CommunityThemeCrypto _crypto(String nsec, String pubkey) {
  final privateHex = nostr.Nip19.decode(payload: nsec).data;
  final key = getConversationKey(privateHex, pubkey);
  return CommunityThemeCrypto(
    encrypt: (plaintext) => nip44Encrypt(key, plaintext),
    decrypt: (ciphertext) => nip44Decrypt(key, ciphertext),
  );
}

final communityThemeStorageProvider = Provider<CommunityThemeStorage>(
  (ref) => CommunityThemeStorage(ref.watch(savedPrefsProvider)),
);

final communityThemeProvider =
    NotifierProvider<CommunityThemeNotifier, CommunityThemePreference>(
      CommunityThemeNotifier.new,
    );
