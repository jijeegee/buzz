import 'dart:convert';

import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:http/http.dart' as http;
import 'package:nostr/nostr.dart' as nostr;

import '../community/community.dart';
import '../community/community_accounts.dart';
import '../community/community_provider.dart';
import '../community/community_storage.dart';
import '../device_key/device_proof.dart';
import '../relay/service_address.dart';
import 'token/token.dart';
import '../l10n/l10n.dart';

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

// User-visible copy: plain English, no key or server jargon (product rule).
KeyBackupException get _tryAgain =>
    KeyBackupException(appL10n.googleBackupConnectFailed);

/// Keeps an uncommitted client key across uncertain uploads and app restarts.
/// It is never the active messaging identity until initialization succeeds,
/// and is gone once the account's row holds the key or the account signs out.
class PendingBackupKeyStore {
  PendingBackupKeyStore({FlutterSecureStorage? storage})
    : _storage = storage ?? const FlutterSecureStorage();
  final FlutterSecureStorage _storage;
  String _key(String origin, String account) =>
      'kuzz.key-backup.pending.v1:${jsonEncode([storageScopeFor(origin), account])}';
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
    required CommunityStorage communities,
    required PendingBackupKeyStore pending,
    nostr.Keys Function()? generateKeys,
  }) : _client = client,
       _communities = communities,
       _pending = pending,
       _generateKeys = generateKeys ?? nostr.Keys.generate;

  final String origin;
  final http.Client _client;
  final CommunityStorage _communities;
  final PendingBackupKeyStore _pending;
  final nostr.Keys Function() _generateKeys;
  final Map<TokenSessionController, Future<Community>> _inFlight = {};
  static final _hexKey = RegExp(r'^[0-9a-f]{64}$');

  /// Resolve and return a signed-key community with [session], the sign-in
  /// attempt: nothing here touches a stored session. The caller persists the
  /// result through AuthNotifier before exposing the authenticated app.
  /// Single-flight per attempt: another attempt is another account's sign-in.
  ///
  /// The row is the one of this account (see [findKeyAccountRow]). Its key
  /// never goes to another account, and a key already paired with the account
  /// always wins over the row's: the row's is discarded. An account with no
  /// row gets a new one with a fresh key.
  ///
  /// With [connect] (Settings, "Connect Google") the row is the active one,
  /// and its key is linked to this account. Only this path links a key.
  Future<Community> resolve({
    required TokenSessionController session,
    bool connect = false,
    bool Function()? isCurrent,
  }) => _inFlight[session] ??= _resolve(session, connect, isCurrent)
      .onError((Object error, StackTrace stack) {
        if (error is KeyBackupException) throw error;
        throw _tryAgain;
      })
      .whenComplete(() {
        _inFlight.remove(session);
      });

  Future<Community> _resolve(
    TokenSessionController session,
    bool connect,
    bool Function()? isCurrent,
  ) async {
    final generation = session.generation;
    void guard() {
      if (isCurrent?.call() == false ||
          generation != session.generation ||
          session.state.status != TokenSessionStatus.signedIn) {
        throw KeyBackupException(appL10n.googleBackupSignInChanged);
      }
    }

    guard();
    _validateTransport();
    if (session.identityMode != 'key_backup') {
      throw KeyBackupException(appL10n.googleBackupSignInWithGoogleFirst);
    }
    final account = session.state.principalId;
    if (account == null) {
      throw KeyBackupException(appL10n.googleBackupSignInAgain);
    }
    final status = await _status(session, account, guard);
    // A key-only community records no Google account yet: its pubkey is only
    // known from the backup status.
    final accountKey = status['pubkey'] as String?;
    Future<Community?> row() => connect
        ? _connectRow(account, accountKey)
        : _accountRow(account, accountKey);
    final existing = await row();
    late nostr.Keys keys;
    if (status['state'] == 'ready') {
      keys = await _restore(session, status, guard);
    } else {
      final local = existing == null ? null : _rowKey(existing);
      // Only a verified successful absent status authorizes key generation.
      final pending = await _pending.read(origin, account);
      guard();
      final candidate =
          local ?? (pending == null ? _generateKeys() : _keys(pending));
      if (candidate == null) {
        throw _tryAgain;
      }
      if (local == null) {
        await _pending.write(origin, account, candidate.nsec);
        guard();
      }
      try {
        await _initialize(session, account, candidate, guard);
        keys = candidate;
      } on KeyBackupException catch (error) {
        if (error.statusCode != 409 ||
            error.code != 'backup_exists' ||
            existing != null) {
          rethrow;
        }
        // Another first device won. Never retry initialization with a new key.
        final winner = await _status(session, account, guard);
        if (winner['state'] != 'ready') rethrow;
        keys = await _restore(session, winner, guard);
      }
    }
    if (session.state.principalId != account ||
        session.identityMode != 'key_backup') {
      throw KeyBackupException(appL10n.googleBackupAccountChanged);
    }
    final now = await row();
    guard();
    if (now?.id != existing?.id ||
        now?.nsec != existing?.nsec ||
        now?.pubkey != existing?.pubkey ||
        now?.tokenAuth != existing?.tokenAuth ||
        now?.signedOut != existing?.signedOut) {
      throw KeyBackupException(appL10n.googleBackupCommunityChanged);
    }
    return existing
            ?.withCredentials(pubkey: keys.public, nsec: keys.nsec)
            .copyWith(googleBackupAccountId: account) ??
        Community.create(
          name: Community.nameFromUrl(origin),
          relayUrl: origin,
          pubkey: keys.public,
          nsec: keys.nsec,
          googleBackupAccountId: account,
        );
  }

  Future<Community?> _accountRow(String account, String? pubkey) async =>
      findKeyAccountRow(
        await _communities.loadAll(),
        origin: origin,
        account: account,
        pubkey: pubkey,
      );

  /// The active row, when [account] may be connected to it: it has its own
  /// key, no other Google account, and [pairedKey] (the key already paired
  /// with [account], if any) is that key.
  Future<Community> _connectRow(String account, String? pairedKey) async {
    final activeId = await _communities.loadActiveId();
    final active = communitiesOfService(
      await _communities.loadAll(),
      origin,
    ).where((community) => community.id == activeId).firstOrNull;
    if (active == null || active.tokenAuth) {
      throw KeyBackupException(appL10n.googleBackupCommunitySignInFailed);
    }
    final linked = active.googleBackupAccountId;
    if (linked != null && linked != account) {
      throw KeyBackupException(appL10n.googleBackupCommunityLinkedElsewhere);
    }
    if (pairedKey != null && pairedKey != active.accountPubkey) {
      throw KeyBackupException(appL10n.googleBackupAccountLinkedElsewhere);
    }
    return active;
  }

  /// The key [community] goes on with when its account has none on the
  /// server yet. A row without a key cannot be helped from here.
  nostr.Keys _rowKey(Community community) {
    final nsec = community.nsec;
    if (community.signedOut || nsec == null || nsec.isEmpty) {
      throw KeyBackupException(appL10n.googleBackupRestoreFailed);
    }
    final keys = _keys(nsec);
    if (keys == null ||
        (community.pubkey != null && community.pubkey != keys.public)) {
      throw KeyBackupException(appL10n.googleBackupCommunitySignInFailed);
    }
    return keys;
  }

  Future<Map<String, dynamic>> _status(
    TokenSessionController session,
    String account,
    void Function() guard,
  ) async {
    final status = await _request(
      session,
      'GET',
      '/auth/key-backup',
      guard: guard,
    );
    if (status['account_id'] != account ||
        status['version'] != 1 ||
        !(status['state'] == 'absent' && status['pubkey'] == null ||
            status['state'] == 'ready' && _validHex(status['pubkey']))) {
      throw _tryAgain;
    }
    return status;
  }

  Future<nostr.Keys> _restore(
    TokenSessionController session,
    Map<String, dynamic> status,
    void Function() guard,
  ) async {
    final result = await _request(
      session,
      'POST',
      '/auth/key-backup/restore',
      guard: guard,
      body: {},
    );
    final secret = result['secret_key'];
    if (!_validHex(secret) ||
        result['version'] != 1 ||
        result['pubkey'] != status['pubkey']) {
      throw _tryAgain;
    }
    final nostr.Keys keys;
    try {
      keys = nostr.Keys(secret as String);
    } catch (_) {
      throw _tryAgain;
    }
    if (keys.public != status['pubkey']) {
      throw _tryAgain;
    }
    return keys;
  }

  Future<void> _initialize(
    TokenSessionController session,
    String account,
    nostr.Keys keys,
    void Function() guard,
  ) async {
    final challenge = await _request(
      session,
      'POST',
      '/auth/key-backup/challenge',
      guard: guard,
      body: {'pubkey': keys.public},
    );
    if (challenge['account_id'] != account ||
        challenge['url'] != '$origin/auth/key-backup' ||
        !_validHex(challenge['challenge'])) {
      throw _tryAgain;
    }
    final proof = await signDeviceTaggedEvent(
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
      session,
      'POST',
      '/auth/key-backup',
      guard: guard,
      body: {'secret_key': keys.secret, 'proof': proof.toMap()},
    );
    if (result['pubkey'] != keys.public || result['version'] != 1) {
      throw _tryAgain;
    }
  }

  Future<Map<String, dynamic>> _request(
    TokenSessionController session,
    String method,
    String path, {
    Map<String, dynamic>? body,
    required void Function() guard,
  }) async {
    guard();
    final token = await session.ensureFreshAccessToken();
    guard();
    if (token == null) {
      throw KeyBackupException(appL10n.googleBackupSignInWithGoogleAgain);
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
        throw _tryAgain;
      }
      if (response.statusCode != 200) {
        final code = decoded['code'];
        throw KeyBackupException(
          switch (code) {
            'reauth_required' => appL10n.googleBackupSignInWithGoogleAgain,
            'key_already_linked' =>
              appL10n.googleBackupCommunityLinkedElsewhere,
            'account_mode_conflict' => appL10n.googleBackupAccountModeConflict,
            _ => _tryAgain.message,
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
      throw _tryAgain;
    }
  }

  Future<String> _readBounded(http.StreamedResponse response) async {
    final bytes = <int>[];
    await for (final chunk in response.stream) {
      if (bytes.length + chunk.length > 16384) {
        throw _tryAgain;
      }
      bytes.addAll(chunk);
    }
    return utf8.decode(bytes);
  }

  void _validateTransport() {
    final uri = Uri.parse(origin);
    const allowLocal = bool.fromEnvironment(
      'KUZZ_ALLOW_INSECURE_LOCAL_KEY_BACKUP',
    );
    if (uri.scheme != 'https' &&
        !(allowLocal &&
            uri.scheme == 'http' &&
            ['localhost', '127.0.0.1', '::1'].contains(uri.host))) {
      throw KeyBackupException(appL10n.googleBackupSecureConnectionRequired);
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
