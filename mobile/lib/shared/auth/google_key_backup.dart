import 'dart:convert';

import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:http/http.dart' as http;
import 'package:nostr/nostr.dart' as nostr;

import '../community/community.dart';
import '../community/community_provider.dart';
import '../community/community_storage.dart';
import 'token/token.dart';

/// A safe, body-free explanation of a custody operation failure.
class KeyBackupException implements Exception {
  const KeyBackupException(this.message, {this.code, this.statusCode});
  final String message;
  final String? code;

  /// HTTP status, when known; only a confirmed 409 permits winner recovery.
  final int? statusCode;
  @override
  String toString() => message;
}

/// Keeps an uncommitted client key across uncertain uploads and app restarts.
/// It is never the active messaging identity until initialization succeeds.
class PendingBackupKeyStore {
  PendingBackupKeyStore({FlutterSecureStorage? storage})
    : _storage = storage ?? const FlutterSecureStorage();
  final FlutterSecureStorage _storage;
  String _key(String origin, String account) =>
      'buzz.key-backup.pending.v1:${jsonEncode([origin, account])}';
  Future<String?> read(String origin, String account) =>
      _storage.read(key: _key(origin, account));
  Future<void> write(String origin, String account, String nsec) =>
      _storage.write(key: _key(origin, account), value: nsec);
  Future<void> delete(String origin, String account) =>
      _storage.delete(key: _key(origin, account));
}

final pendingBackupKeyStoreProvider = Provider<PendingBackupKeyStore>(
  (ref) => PendingBackupKeyStore(),
);

final googleKeyBackupServiceProvider =
    Provider.family<GoogleKeyBackupService, String>((ref, origin) {
      return GoogleKeyBackupService(
        origin: origin,
        client: ref.watch(authHttpClientProvider),
        session: ref.watch(keyBackupSessionControllerProvider(origin)),
        communities: ref.watch(communityStorageProvider),
        pending: ref.watch(pendingBackupKeyStoreProvider),
      );
    });

/// Resolves a Google account to the original client-generated signing key.
/// Uses Google sessions solely for backup authorization; never token messages.
class GoogleKeyBackupService {
  GoogleKeyBackupService({
    required this.origin,
    required http.Client client,
    required TokenSessionController session,
    required CommunityStorage communities,
    required PendingBackupKeyStore pending,
    nostr.Keys Function()? generateKeys,
  }) : _client = client,
       _session = session,
       _communities = communities,
       _pending = pending,
       _generateKeys = generateKeys ?? nostr.Keys.generate;

  final String origin;
  final http.Client _client;
  final TokenSessionController _session;
  final CommunityStorage _communities;
  final PendingBackupKeyStore _pending;
  final nostr.Keys Function() _generateKeys;
  Future<Community>? _inFlight;
  static final _hexKey = RegExp(r'^[0-9a-f]{64}$');

  /// Check before OIDC can replace the origin-scoped session record.
  Future<void> checkLocalCompatibility() async {
    _validateTransport();
    final Community? existing;
    try {
      existing = await _existing();
    } catch (_) {
      throw const KeyBackupException(
        'Could not read the saved identity securely. Try again.',
      );
    }
    if (existing?.tokenAuth == true) {
      throw const KeyBackupException(
        'This device has a separate token account on this relay. '
        'It cannot be merged with a signing-key account. Its data is unchanged.',
      );
    }
  }

  /// Resolve and return a signed-key community. The caller persists it through
  /// AuthNotifier before exposing the authenticated app. Single-flight.
  Future<Community> resolve({bool Function()? isCurrent}) =>
      _inFlight ??= _resolve(isCurrent)
          .onError((Object error, StackTrace stack) {
            if (error is KeyBackupException) throw error;
            throw const KeyBackupException(
              'Could not read or save the signing key securely. '
              'Your existing identity is unchanged; try again.',
            );
          })
          .whenComplete(() {
            _inFlight = null;
          });

  Future<Community> _resolve(bool Function()? isCurrent) async {
    final generation = _session.generation;
    void guard() {
      if (isCurrent?.call() == false ||
          generation != _session.generation ||
          _session.state.status != TokenSessionStatus.signedIn) {
        throw const KeyBackupException(
          'Sign-in changed or was cancelled. Try again.',
        );
      }
    }

    guard();
    await checkLocalCompatibility();
    guard();
    if (_session.identityMode != 'key_backup') {
      throw const KeyBackupException(
        'Sign in with Google for key recovery first.',
      );
    }
    final account = _session.state.principalId;
    if (account == null) throw const KeyBackupException('Sign in again.');
    final existing = await _existing();
    final local = existing == null ? null : _keys(existing.nsec);
    final missingLocalKey =
        existing != null && (existing.nsec == null || existing.nsec!.isEmpty);
    if (existing != null &&
        ((missingLocalKey && !_validHex(existing.pubkey)) ||
            (!missingLocalKey && local == null) ||
            (local != null &&
                existing.pubkey != null &&
                existing.pubkey != local.public))) {
      throw const KeyBackupException(
        'The saved identity is inconsistent. '
        'Recover its original key before linking Google; nothing was replaced.',
      );
    }
    final status = await _status(account, guard);
    late nostr.Keys keys;
    if (status['state'] == 'ready') {
      if (existing?.pubkey != null && existing!.pubkey != status['pubkey']) {
        throw const KeyBackupException(
          'This Google backup belongs to a different '
          'Buzz identity. The saved identity was preserved.',
        );
      }
      keys = await _restore(status, local, guard);
    } else {
      if (missingLocalKey) {
        throw const KeyBackupException(
          'This Google account has no backup for '
          'the saved identity. Recover its original key; no replacement was generated.',
        );
      }
      // Only a verified successful absent status authorizes key generation.
      final pending = await _pending.read(origin, account);
      guard();
      final candidate =
          local ?? (pending == null ? _generateKeys() : _keys(pending));
      if (candidate == null) {
        throw const KeyBackupException(
          'The pending key cannot be read. '
          'No replacement identity was generated.',
        );
      }
      await _pending.write(origin, account, candidate.nsec);
      guard();
      try {
        await _initialize(account, candidate, guard);
        keys = candidate;
      } on KeyBackupException catch (error) {
        if (error.statusCode != 409 ||
            error.code != 'backup_exists' ||
            existing != null) {
          rethrow;
        }
        // Another first device won. Never retry initialization with a new key.
        final winner = await _status(account, guard);
        if (winner['state'] != 'ready') rethrow;
        keys = await _restore(winner, null, guard);
      }
    }
    if (_session.state.principalId != account ||
        _session.identityMode != 'key_backup') {
      throw const KeyBackupException(
        'The Google account changed. Sign in again.',
      );
    }
    final now = await _existing();
    guard();
    if (now?.id != existing?.id ||
        now?.nsec != existing?.nsec ||
        now?.pubkey != existing?.pubkey ||
        now?.tokenAuth != existing?.tokenAuth) {
      throw const KeyBackupException(
        'This community changed during sign-in. '
        'Its identity was preserved; try again.',
      );
    }
    return existing?.copyWith(
          googleBackupAccountId: account,
          pubkey: keys.public,
          nsec: keys.nsec,
        ) ??
        Community.create(
          name: Community.nameFromUrl(origin),
          relayUrl: origin,
          pubkey: keys.public,
          nsec: keys.nsec,
          googleBackupAccountId: account,
        );
  }

  Future<Community?> _existing() async {
    final all = await _communities.loadAll();
    return all
        .where(
          (community) => normalizeRelayOrigin(community.relayUrl) == origin,
        )
        .firstOrNull;
  }

  Future<Map<String, dynamic>> _status(
    String account,
    void Function() guard,
  ) async {
    final status = await _request('GET', '/auth/key-backup', guard: guard);
    if (status['account_id'] != account ||
        status['version'] != 1 ||
        !(status['state'] == 'absent' && status['pubkey'] == null ||
            status['state'] == 'ready' && _validHex(status['pubkey']))) {
      throw const KeyBackupException(
        'The relay returned an invalid backup status.',
      );
    }
    return status;
  }

  Future<nostr.Keys> _restore(
    Map<String, dynamic> status,
    nostr.Keys? local,
    void Function() guard,
  ) async {
    if (local != null && local.public != status['pubkey']) {
      throw const KeyBackupException(
        'This Google account backs up another Buzz '
        'identity. Your existing identity and conversations were preserved.',
      );
    }
    final result = await _request(
      'POST',
      '/auth/key-backup/restore',
      guard: guard,
      body: {},
    );
    final secret = result['secret_key'];
    if (!_validHex(secret) ||
        result['version'] != 1 ||
        result['pubkey'] != status['pubkey']) {
      throw const KeyBackupException(
        'The restored backup is invalid. No key was replaced.',
      );
    }
    final nostr.Keys keys;
    try {
      keys = nostr.Keys(secret as String);
    } catch (_) {
      throw const KeyBackupException(
        'The restored key is invalid. No key was replaced.',
      );
    }
    if (keys.public != status['pubkey']) {
      throw const KeyBackupException(
        'The restored key does not match its identity.',
      );
    }
    return keys;
  }

  Future<void> _initialize(
    String account,
    nostr.Keys keys,
    void Function() guard,
  ) async {
    final challenge = await _request(
      'POST',
      '/auth/key-backup/challenge',
      guard: guard,
      body: {'pubkey': keys.public},
    );
    if (challenge['account_id'] != account ||
        challenge['url'] != '$origin/auth/key-backup' ||
        !_validHex(challenge['challenge'])) {
      throw const KeyBackupException(
        'The relay returned an invalid linking challenge.',
      );
    }
    final proof = nostr.Event.from(
      kind: 27235,
      content: '',
      secretKey: keys.secret,
      tags: [
        ['u', challenge['url'] as String],
        ['method', 'POST'],
        ['challenge', challenge['challenge'] as String],
        ['account', account],
        ['action', 'initialize'],
      ],
    );
    final result = await _request(
      'POST',
      '/auth/key-backup',
      guard: guard,
      body: {'secret_key': keys.secret, 'proof': proof.toMap()},
    );
    if (result['pubkey'] != keys.public || result['version'] != 1) {
      throw const KeyBackupException(
        'The relay did not confirm this key backup.',
      );
    }
  }

  Future<Map<String, dynamic>> _request(
    String method,
    String path, {
    Map<String, dynamic>? body,
    required void Function() guard,
  }) async {
    guard();
    final token = await _session.ensureFreshAccessToken();
    guard();
    if (token == null) {
      throw const KeyBackupException('Sign in with Google again.');
    }
    try {
      final request = http.Request(method, Uri.parse('$origin$path'))
        ..followRedirects = false
        ..headers.addAll({
          'Authorization': 'Bearer $token',
          'Content-Type': 'application/json',
          'Cache-Control': 'no-store',
        });
      if (body != null) request.body = jsonEncode(body);
      final response = await _client
          .send(request)
          .timeout(const Duration(seconds: 20));
      final raw = await _readBounded(
        response,
      ).timeout(const Duration(seconds: 20));
      guard();
      final decoded = jsonDecode(raw);
      if (decoded is! Map<String, dynamic>) {
        throw const KeyBackupException(
          'The relay returned an invalid backup response.',
        );
      }
      if (response.statusCode != 200) {
        final code = decoded['code'];
        throw KeyBackupException(
          switch (code) {
            'reauth_required' =>
              'Sign in with Google again to recover or back up your key.',
            'backup_exists' =>
              'This account already has an immutable key backup.',
            'key_already_linked' =>
              'This key is already linked to another Google account.',
            'account_mode_conflict' =>
              'This Google account has separate token-mode data. It cannot be merged.',
            _ =>
              'Key backup request failed (HTTP ${response.statusCode}). Try again; your identity is unchanged.',
          },
          code: code is String ? code : null,
          statusCode: response.statusCode,
        );
      }
      return decoded;
    } on KeyBackupException {
      rethrow;
    } catch (_) {
      // Never include a key-bearing response/body or a transport exception.
      throw const KeyBackupException(
        'Could not complete key recovery. '
        'Try again; no replacement identity was generated.',
      );
    }
  }

  Future<String> _readBounded(http.StreamedResponse response) async {
    final bytes = <int>[];
    await for (final chunk in response.stream) {
      if (bytes.length + chunk.length > 16384) {
        throw const KeyBackupException('The backup response was too large.');
      }
      bytes.addAll(chunk);
    }
    return utf8.decode(bytes);
  }

  void _validateTransport() {
    final uri = Uri.parse(origin);
    const allowLocal = bool.fromEnvironment(
      'BUZZ_ALLOW_INSECURE_LOCAL_KEY_BACKUP',
    );
    if (uri.scheme != 'https' &&
        !(allowLocal &&
            uri.scheme == 'http' &&
            ['localhost', '127.0.0.1', '::1'].contains(uri.host))) {
      throw const KeyBackupException('Google key recovery requires HTTPS.');
    }
  }

  static bool _validHex(Object? value) =>
      value is String && _hexKey.hasMatch(value);
  static nostr.Keys? _keys(String? nsec) {
    if (nsec == null) return null;
    try {
      final decoded = nostr.Nip19.decode(payload: nsec);
      if (decoded.prefix != nostr.Nip19Prefix.nsec ||
          !_validHex(decoded.data)) {
        return null;
      }
      return nostr.Keys(decoded.data);
    } catch (_) {
      return null;
    }
  }
}
