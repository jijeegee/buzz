import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';

import '../../relay/service_address.dart';
import 'session_key.dart';

/// The durable half of a token session: the refresh token and the principal
/// it belongs to, written together in one record.
class StoredTokenSession {
  const StoredTokenSession({
    required this.refreshToken,
    required this.principalId,
    this.identityMode = 'token',
  });

  final String refreshToken;
  final String principalId;
  final String identityMode;

  @override
  bool operator ==(Object other) =>
      other is StoredTokenSession &&
      other.refreshToken == refreshToken &&
      other.principalId == principalId &&
      other.identityMode == identityMode;

  @override
  int get hashCode => Object.hash(refreshToken, principalId, identityMode);
}

/// Account-scoped durable storage for refresh tokens. Access tokens are
/// never stored.
abstract interface class RefreshTokenStore {
  Future<StoredTokenSession?> read(SessionKey key);

  /// Atomically replace the record for [key]. Throws on failure.
  Future<void> write(SessionKey key, StoredTokenSession session);

  Future<void> delete(SessionKey key);
}

/// [RefreshTokenStore] in memory, for a sign-in attempt: until the account is
/// known there is no record to write, and an abandoned attempt leaves nothing
/// behind.
class MemoryRefreshTokenStore implements RefreshTokenStore {
  final Map<SessionKey, StoredTokenSession> _records = {};

  @override
  Future<StoredTokenSession?> read(SessionKey key) async => _records[key];

  @override
  Future<void> write(SessionKey key, StoredTokenSession session) async =>
      _records[key] = session;

  @override
  Future<void> delete(SessionKey key) async => _records.remove(key);
}

/// [RefreshTokenStore] in the platform keychain/keystore.
class SecureRefreshTokenStore implements RefreshTokenStore {
  SecureRefreshTokenStore({
    FlutterSecureStorage? storage,
    this.keyPrefix = tokenKeyPrefix,
  }) : _storage = storage ?? const FlutterSecureStorage();

  /// Key prefix of the token store.
  static const tokenKeyPrefix = 'kuzz.auth.refresh';

  /// Key prefix of the key-backup store.
  static const keyBackupKeyPrefix = 'kuzz.auth.key-backup.refresh';

  final FlutterSecureStorage _storage;
  final String keyPrefix;

  /// Storage key for one account's record: the service scope for this
  /// build's service (any of its addresses), otherwise the origin itself,
  /// then the account.
  static String keyFor(SessionKey key, {String prefix = tokenKeyPrefix}) =>
      '$prefix.v2:${storageScopeFor(key.origin)}:${key.principal}';

  /// Storage key of the one record per server that came before accounts.
  static String legacyKeyFor(String origin, {String prefix = tokenKeyPrefix}) =>
      '$prefix.v1:${storageScopeFor(origin)}';

  /// Readable after the first unlock (background refresh) but never synced
  /// or restored to another device: a refresh token is bound to this device.
  static const iOptions = IOSOptions(
    accessibility: KeychainAccessibility.first_unlock_this_device,
  );

  /// Reads the record, first moving it out of its server's old record when
  /// it has not moved yet: phones cannot run a script, and asking every user
  /// to sign in again is not acceptable. Runs inside the controller's store
  /// queue, so no rotation write can interleave with the move.
  ///
  /// A record that cannot be decoded can never be used, so it is deleted and
  /// reported as absent (signed out). A keychain that cannot be read throws:
  /// only a value confirmed absent means there is no session.
  @override
  Future<StoredTokenSession?> read(SessionKey key) async {
    final current = await _readRecord(_keyFor(key), key);
    final legacy = await _readRecord(_legacyKeyFor(key.origin), key);
    final legacyIsMine = legacy?.principalId == key.principal;
    if (current != null) {
      if (legacyIsMine) await _tryDeleteLegacyCopies(key);
      return current;
    }
    // Another account's session stays where it is until that account reads.
    if (legacy == null || !legacyIsMine) return null;
    await write(key, legacy);
    if (await _readRecord(_keyFor(key), key) != legacy) {
      throw StateError('A moved token session did not read back');
    }
    await _tryDeleteLegacyCopies(key);
    return legacy;
  }

  @override
  Future<void> write(SessionKey key, StoredTokenSession session) =>
      _storage.write(
        key: _keyFor(key),
        value: jsonEncode({
          'refresh': session.refreshToken,
          'principal_id': session.principalId,
          if (session.identityMode != 'token')
            'identity_mode': session.identityMode,
        }),
        iOptions: iOptions,
      );

  /// Deletes the account's record and any old record it has not moved from:
  /// the next read would otherwise move that one in and undo the sign-out.
  @override
  Future<void> delete(SessionKey key) async {
    await _storage.delete(key: _keyFor(key), iOptions: iOptions);
    final legacy = await _readRecord(_legacyKeyFor(key.origin), key);
    if (legacy?.principalId == key.principal) await _deleteLegacyCopies(key);
  }

  String _keyFor(SessionKey key) => keyFor(key, prefix: keyPrefix);

  String _legacyKeyFor(String origin) =>
      legacyKeyFor(origin, prefix: keyPrefix);

  Future<StoredTokenSession?> _readRecord(String key, SessionKey owner) async {
    final raw = await _storage.read(key: key, iOptions: iOptions);
    if (raw == null) return null;
    final session = _decode(raw);
    if (session == null) {
      debugPrint('Discarding unreadable token session for ${owner.origin}');
      await _storage.delete(key: key, iOptions: iOptions);
    }
    return session;
  }

  /// The account's own record is fine, so a failure here only leaves the old
  /// record for the next read to remove.
  Future<void> _tryDeleteLegacyCopies(SessionKey key) async {
    try {
      await _deleteLegacyCopies(key);
    } catch (error) {
      debugPrint('Could not remove an old token record: $error');
    }
  }

  /// Removes the old record and every address-form key it was copied from.
  ///
  /// Other stored values keep their old keys (`service_scope_migration`
  /// never deletes one). A refresh token is the exception: it is single-use,
  /// so a stale copy presented later would trigger reuse detection and end
  /// the session. Deleting the copies also means that migration can never
  /// bring a spent token back. The scoped record goes last: while it exists,
  /// a later read knows this cleanup is unfinished.
  ///
  /// The names are computed, never listed: on Android `readAll` decrypts
  /// every entry, and one that cannot be decrypted makes the plugin wipe the
  /// whole store (`resetOnError` defaults to true), every account included.
  Future<void> _deleteLegacyCopies(SessionKey key) async {
    final legacyKey = _legacyKeyFor(key.origin);
    final service = appServiceAddresses;
    final copies = <String>{
      if (service.isServiceAddress(key.origin))
        for (final origin in service.origins) ...[
          '$keyPrefix.v1:$origin',
          '$keyPrefix.v1:${origin.replaceFirst('http', 'ws')}',
        ],
    }..remove(legacyKey);
    for (final copy in copies) {
      // The migration's two passes: older records were written without
      // [iOptions], and iOS filters keychain queries by accessibility.
      await _storage.delete(key: copy, iOptions: iOptions);
      await _storage.delete(key: copy);
    }
    await _storage.delete(key: legacyKey, iOptions: iOptions);
  }

  static StoredTokenSession? _decode(String raw) {
    try {
      final json = jsonDecode(raw);
      if (json is! Map) return null;
      final refresh = json['refresh'];
      final principal = json['principal_id'];
      if (refresh is! String || refresh.isEmpty) return null;
      if (principal is! String || principal.isEmpty) return null;
      return StoredTokenSession(
        refreshToken: refresh,
        principalId: principal,
        identityMode: json['identity_mode'] is String
            ? json['identity_mode'] as String
            : 'token',
      );
    } on FormatException {
      return null;
    }
  }
}
