import 'dart:convert';

import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:http/http.dart' as http;

import '../token/token.dart';

/// A failed account request (`/auth/me`, `/auth/profile`, `/auth/devices`,
/// `/auth/sessions/*`, `/auth/bots/*`, `/auth/account`, `/auth/blocks`,
/// `/auth/people/*`).
class AccountApiException implements Exception {
  const AccountApiException(this.message, {this.statusCode, this.code});

  /// Human-readable reason (relay `error` field when present).
  final String message;

  /// HTTP status, or `null` when no response arrived.
  final int? statusCode;

  /// Relay machine code (`username_taken`, `token_expired`, ...).
  final String? code;

  @override
  String toString() => message;
}

/// The signed-in principal's global profile (`GET /auth/me`,
/// `PATCH /auth/profile`).
class AccountProfile {
  const AccountProfile({
    required this.principalId,
    required this.displayName,
    this.avatarUrl,
    this.username,
    this.email,
    this.discoverable = true,
  });

  factory AccountProfile.fromJson(Map<String, Object?> json) {
    final principal = json['principal_id'];
    if (principal is! String || principal.isEmpty) {
      throw const AccountApiException(
        'Malformed relay response: missing principal_id',
      );
    }
    final name = json['display_name'];
    final avatar = json['avatar_url'];
    final username = json['username'];
    final email = json['email'];
    final discoverable = json['discoverable'];
    return AccountProfile(
      principalId: principal,
      displayName: name is String ? name : '',
      avatarUrl: avatar is String && avatar.isNotEmpty ? avatar : null,
      username: username is String && username.isNotEmpty ? username : null,
      email: email is String && email.isNotEmpty ? email : null,
      // Relays before people lookup send none; the account default is on.
      discoverable: discoverable is bool ? discoverable : true,
    );
  }

  final String principalId;
  final String displayName;
  final String? avatarUrl;
  final String? username;

  /// The Google email of this session's account; relays before it was
  /// added to `/auth/me` send none.
  final String? email;

  /// Whether other people can find this account by its exact Kuzz ID or
  /// email (`GET /auth/people/lookup`).
  final bool discoverable;
}

/// Another person as the people endpoints return them
/// (`/auth/people/lookup`, `/auth/people/shared`). Never carries an email.
class PersonSummary {
  const PersonSummary({
    required this.pubkey,
    this.displayName,
    this.avatarUrl,
    this.username,
  });

  factory PersonSummary.fromJson(Map<String, Object?> json) {
    final pubkey = json['pubkey'];
    if (pubkey is! String || !_hexPubkey.hasMatch(pubkey.toLowerCase())) {
      throw const AccountApiException('Malformed relay response: pubkey');
    }
    final name = json['display_name'];
    final avatar = json['avatar_url'];
    final username = json['username'];
    return PersonSummary(
      pubkey: pubkey.toLowerCase(),
      displayName: name is String && name.trim().isNotEmpty ? name : null,
      avatarUrl: avatar is String && avatar.isNotEmpty ? avatar : null,
      username: username is String && username.isNotEmpty ? username : null,
    );
  }

  /// Lowercase hex public key.
  final String pubkey;
  final String? displayName;
  final String? avatarUrl;

  /// The person's Kuzz ID, when they set one.
  final String? username;
}

/// One person this account blocked (`GET /auth/blocks`).
class BlockedPerson {
  const BlockedPerson({required this.pubkey, this.createdAt});

  final String pubkey;
  final DateTime? createdAt;
}

final _hexPubkey = RegExp(r'^[0-9a-f]{64}$');

/// Lowercase [pubkey]; throws [ArgumentError] unless it is 64 hex
/// characters, so a malformed key never reaches a request path.
String _pathPubkey(String pubkey) {
  final normalized = pubkey.trim().toLowerCase();
  if (!_hexPubkey.hasMatch(normalized)) {
    throw ArgumentError.value(pubkey, 'pubkey', 'not a hex public key');
  }
  return normalized;
}

/// One signed-in device (`GET /auth/devices`).
class AccountDevice {
  const AccountDevice({
    required this.id,
    required this.name,
    required this.platform,
    required this.current,
    this.lastSeenAt,
  });

  factory AccountDevice.fromJson(Map<String, Object?> json) {
    final id = json['id'];
    if (id is! String || id.isEmpty) {
      throw const AccountApiException('Malformed relay response: device id');
    }
    final name = json['name'];
    final platform = json['platform'];
    final lastSeen = json['last_seen_at'];
    return AccountDevice(
      id: id,
      name: name is String && name.isNotEmpty ? name : 'Unnamed device',
      platform: platform is String ? platform : '',
      current: json['current'] == true,
      lastSeenAt: lastSeen is String ? DateTime.tryParse(lastSeen) : null,
    );
  }

  final String id;
  final String name;

  /// `desktop`, `mobile`, `web` or `cli`.
  final String platform;

  /// Whether this is the device making the request.
  final bool current;
  final DateTime? lastSeenAt;
}

/// The account endpoints of one token relay, authenticated with the
/// session's Bearer access token.
class AccountApi {
  AccountApi({
    required this.origin,
    required http.Client client,
    required TokenSessionController session,
    this.timeout = const Duration(seconds: 20),
  }) : _client = client,
       _session = session;

  final String origin;
  final http.Client _client;
  final TokenSessionController _session;
  final Duration timeout;

  Future<AccountProfile> me() async {
    final response = await _send('GET', '/auth/me');
    return AccountProfile.fromJson(_object(response));
  }

  /// Save the whole profile in one request: a single atomic write on the
  /// relay. `null` [avatarUrl]/[username] clear the value.
  Future<AccountProfile> updateProfile({
    required String displayName,
    required String? avatarUrl,
    required String? username,
  }) async {
    final response = await _send(
      'PATCH',
      '/auth/profile',
      body: {
        'display_name': displayName,
        'avatar_url': avatarUrl,
        'username': username,
      },
    );
    return AccountProfile.fromJson(_object(response));
  }

  /// Set or clear (`null`) the Kuzz ID alone; other profile fields keep
  /// their value. A taken ID fails with code `username_taken` (409).
  Future<AccountProfile> setUsername(String? username) async {
    final response = await _send(
      'PATCH',
      '/auth/profile',
      body: {'username': username},
    );
    return AccountProfile.fromJson(_object(response));
  }

  /// Whether people can find this account by exact Kuzz ID or email.
  Future<void> setDiscoverable(bool discoverable) =>
      _send('PATCH', '/auth/profile', body: {'discoverable': discoverable});

  /// The people this account blocked; only the account itself can read it.
  Future<List<BlockedPerson>> listBlocks() async {
    final response = await _send('GET', '/auth/blocks');
    final blocked = _object(response)['blocked'];
    if (blocked is! List) {
      throw const AccountApiException('Malformed relay response: blocked');
    }
    final result = <BlockedPerson>[];
    for (final item in blocked) {
      if (item is! Map<String, Object?>) continue;
      final pubkey = item['pubkey'];
      if (pubkey is! String || !_hexPubkey.hasMatch(pubkey.toLowerCase())) {
        continue;
      }
      final createdAt = item['created_at'];
      result.add(
        BlockedPerson(
          pubkey: pubkey.toLowerCase(),
          createdAt: createdAt is String ? DateTime.tryParse(createdAt) : null,
        ),
      );
    }
    return result;
  }

  /// Block [pubkey] (idempotent).
  Future<void> block(String pubkey) =>
      _send('PUT', '/auth/blocks/${_pathPubkey(pubkey)}');

  /// Unblock [pubkey] (idempotent).
  Future<void> unblock(String pubkey) =>
      _send('DELETE', '/auth/blocks/${_pathPubkey(pubkey)}');

  /// The one discoverable person whose Kuzz ID or email is exactly [query],
  /// or `null` when there is none (404). Too many lookups fail with 429.
  /// A leading `@` on an ID ("@ada") is dropped: the relay reads any `@` as
  /// an email address.
  Future<PersonSummary?> lookupPerson(String query) async {
    var q = query.trim();
    if (q.startsWith('@') && !q.substring(1).contains('@')) {
      q = q.substring(1);
    }
    final http.Response response;
    try {
      response = await _send('GET', '/auth/people/lookup', query: {'q': q});
    } on AccountApiException catch (error) {
      if (error.statusCode == 404) return null;
      rethrow;
    }
    return PersonSummary.fromJson(_object(response));
  }

  /// People in this account's channels whose name or Kuzz ID starts with
  /// [query]; at most [limit], which the relay caps at 50.
  Future<List<PersonSummary>> sharedPeople(
    String query, {
    int limit = 50,
  }) async {
    final response = await _send(
      'GET',
      '/auth/people/shared',
      query: {'q': query.trim(), 'limit': '${limit.clamp(1, 50)}'},
    );
    final people = _object(response)['people'];
    if (people is! List) {
      throw const AccountApiException('Malformed relay response: people');
    }
    final result = <PersonSummary>[];
    for (final item in people) {
      if (item is! Map<String, Object?>) continue;
      try {
        result.add(PersonSummary.fromJson(item));
      } on AccountApiException {
        continue;
      }
    }
    return result;
  }

  Future<List<AccountDevice>> listDevices() async {
    final response = await _send('GET', '/auth/devices');
    final decoded = _decode(response.body);
    if (decoded is! List) {
      throw const AccountApiException('Malformed relay response: devices');
    }
    return [
      for (final item in decoded)
        if (item is Map<String, Object?>) AccountDevice.fromJson(item),
    ];
  }

  /// Sign [deviceId] out remotely (revokes its session).
  Future<void> revokeDevice(String deviceId) =>
      _send('DELETE', '/auth/devices/${Uri.encodeComponent(deviceId)}');

  /// Rename [deviceId] (any of this account's devices).
  Future<AccountDevice> renameDevice(String deviceId, String name) async {
    final response = await _send(
      'PATCH',
      '/auth/devices/${Uri.encodeComponent(deviceId)}',
      body: {'name': name},
    );
    return AccountDevice.fromJson(_object(response));
  }

  /// Sign out every other human session; bots are untouched.
  Future<void> revokeOtherSessions() =>
      _send('POST', '/auth/sessions/revoke-others');

  Future<void> revokeAllBotTokens() => _send('POST', '/auth/bots/revoke-all');

  /// Disable the account now (purged after 30 days unless signed in again).
  /// Also ends this session on the relay.
  Future<void> deleteAccount() => _send('DELETE', '/auth/account');

  Future<http.Response> _send(
    String method,
    String path, {
    Map<String, Object?>? body,
    Map<String, String>? query,
  }) async {
    // A session nobody has restored yet (the gate normally does) has no
    // refresh token in memory: restore it rather than report signed out.
    if (_session.state.status == TokenSessionStatus.restoring) {
      await _session.restore();
    }
    var token = await _session.ensureFreshAccessToken();
    if (token == null) throw _signedOut();
    var response = await _request(method, path, token, body, query);
    if (response.statusCode == 401 && _code(response) == 'token_expired') {
      token = await _session.refreshAfterTokenExpired(token);
      if (token == null) throw _signedOut();
      response = await _request(method, path, token, body, query);
    }
    final status = response.statusCode;
    if (status >= 200 && status < 300) return response;
    final code = _code(response);
    if (code == 'token_revoked' || code == 'principal_disabled') {
      // The session is dead on the relay. Presenting the refresh token lets
      // the session controller learn that (a terminal refresh signs it out
      // and the gate shows sign-in) instead of staying "signed in" until the
      // next scheduled rotation.
      await _session.refreshAfterTokenExpired(token);
    }
    final error = _decode(response.body);
    final message = error is Map<String, Object?> ? error['error'] : null;
    throw AccountApiException(
      message is String && message.isNotEmpty ? message : 'HTTP $status',
      statusCode: status,
      code: code,
    );
  }

  Future<http.Response> _request(
    String method,
    String path,
    String token,
    Map<String, Object?>? body,
    Map<String, String>? query,
  ) async {
    final uri = Uri.parse('$origin$path');
    final request = http.Request(
      method,
      query == null ? uri : uri.replace(queryParameters: query),
    )..headers['Authorization'] = 'Bearer $token';
    if (body != null) {
      request.headers['Content-Type'] = 'application/json';
      request.body = jsonEncode(body);
    }
    try {
      final streamed = await _client.send(request).timeout(timeout);
      return await http.Response.fromStream(streamed).timeout(timeout);
    } catch (error) {
      throw AccountApiException('Could not reach the relay: $error');
    }
  }

  static AccountApiException _signedOut() =>
      const AccountApiException('You are signed out of this relay.');

  static String? _code(http.Response response) {
    final decoded = _decode(response.body);
    final code = decoded is Map<String, Object?> ? decoded['code'] : null;
    return code is String ? code : null;
  }

  static Map<String, Object?> _object(http.Response response) {
    final decoded = _decode(response.body);
    if (decoded is Map<String, Object?>) return decoded;
    throw const AccountApiException('Malformed relay response');
  }

  static Object? _decode(String body) {
    if (body.isEmpty) return null;
    try {
      return jsonDecode(body);
    } on FormatException {
      return null;
    }
  }
}

/// Account endpoints for the token relay at `origin`.
final accountApiProvider = Provider.family<AccountApi, String>((ref, origin) {
  final googleKey = ref.watch(activeGoogleKeyOriginProvider) == origin;
  return AccountApi(
    origin: origin,
    client: ref.watch(authHttpClientProvider),
    session: ref.watch(
      googleKey
          ? keyBackupSessionControllerProvider(origin)
          : tokenSessionControllerProvider(origin),
    ),
  );
});

/// No automatic retry: account pages show the failure with a Try again
/// action instead of silently re-sending requests.
Duration? _noRetry(int retryCount, Object error) => null;

/// The signed-in principal's global profile. Invalidate to reload.
final accountProfileProvider = FutureProvider.autoDispose
    .family<AccountProfile, String>(
      (ref, origin) => ref.watch(accountApiProvider(origin)).me(),
      retry: _noRetry,
    );

/// The account origin of the active community: its token relay, or the
/// relay holding its Google key backup. `null` when the community has no
/// account (an older key-only community), which hides account features.
final accountOriginProvider = Provider<String?>(
  (ref) =>
      ref.watch(activeTokenOriginProvider) ??
      ref.watch(activeGoogleKeyOriginProvider),
);

/// This account's signed-in devices. Invalidate to reload.
final accountDevicesProvider = FutureProvider.autoDispose
    .family<List<AccountDevice>, String>(
      (ref, origin) => ref.watch(accountApiProvider(origin)).listDevices(),
      retry: _noRetry,
    );
