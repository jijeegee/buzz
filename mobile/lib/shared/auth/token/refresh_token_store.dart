import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';

/// The durable half of a token session: the refresh token and the principal
/// it belongs to, written together in one record.
class StoredTokenSession {
  const StoredTokenSession({
    required this.refreshToken,
    required this.principalId,
  });

  final String refreshToken;
  final String principalId;

  @override
  bool operator ==(Object other) =>
      other is StoredTokenSession &&
      other.refreshToken == refreshToken &&
      other.principalId == principalId;

  @override
  int get hashCode => Object.hash(refreshToken, principalId);
}

/// Origin-scoped durable storage for refresh tokens. Access tokens are
/// never stored.
abstract interface class RefreshTokenStore {
  Future<StoredTokenSession?> read(String origin);

  /// Atomically replace the record for [origin]. Throws on failure.
  Future<void> write(String origin, StoredTokenSession session);

  Future<void> delete(String origin);
}

/// [RefreshTokenStore] in the platform keychain/keystore.
class SecureRefreshTokenStore implements RefreshTokenStore {
  SecureRefreshTokenStore({FlutterSecureStorage? storage})
    : _storage = storage ?? const FlutterSecureStorage();

  final FlutterSecureStorage _storage;

  /// Storage key for [origin]'s record.
  static String keyFor(String origin) => 'buzz.auth.refresh.v1:$origin';

  /// Readable after the first unlock (background refresh) but never synced
  /// or restored to another device: a refresh token is bound to this device.
  static const _iOptions = IOSOptions(
    accessibility: KeychainAccessibility.first_unlock_this_device,
  );

  /// Reads the record. A record that cannot be decoded can never be used,
  /// so it is deleted and reported as absent (signed out).
  @override
  Future<StoredTokenSession?> read(String origin) async {
    final key = keyFor(origin);
    final raw = await _storage.read(key: key, iOptions: _iOptions);
    if (raw == null) return null;
    final session = _decode(raw);
    if (session == null) {
      debugPrint('Discarding unreadable token session for $origin');
      await _storage.delete(key: key, iOptions: _iOptions);
    }
    return session;
  }

  @override
  Future<void> write(String origin, StoredTokenSession session) =>
      _storage.write(
        key: keyFor(origin),
        value: jsonEncode({
          'refresh': session.refreshToken,
          'principal_id': session.principalId,
        }),
        iOptions: _iOptions,
      );

  @override
  Future<void> delete(String origin) =>
      _storage.delete(key: keyFor(origin), iOptions: _iOptions);

  static StoredTokenSession? _decode(String raw) {
    try {
      final json = jsonDecode(raw);
      if (json is! Map) return null;
      final refresh = json['refresh'];
      final principal = json['principal_id'];
      if (refresh is! String || refresh.isEmpty) return null;
      if (principal is! String || principal.isEmpty) return null;
      return StoredTokenSession(refreshToken: refresh, principalId: principal);
    } on FormatException {
      return null;
    }
  }
}
