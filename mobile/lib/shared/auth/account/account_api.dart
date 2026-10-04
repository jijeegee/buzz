import 'dart:convert';

import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:http/http.dart' as http;

import '../token/token.dart';

/// A failed account request (`/auth/me`, `/auth/profile`, `/auth/devices`,
/// `/auth/sessions/*`, `/auth/bots/*`, `/auth/account`).
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
    return AccountProfile(
      principalId: principal,
      displayName: name is String ? name : '',
      avatarUrl: avatar is String && avatar.isNotEmpty ? avatar : null,
      username: username is String && username.isNotEmpty ? username : null,
    );
  }

  final String principalId;
  final String displayName;
  final String? avatarUrl;
  final String? username;
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
  }) async {
    // A session nobody has restored yet (the gate normally does) has no
    // refresh token in memory: restore it rather than report signed out.
    if (_session.state.status == TokenSessionStatus.restoring) {
      await _session.restore();
    }
    var token = await _session.ensureFreshAccessToken();
    if (token == null) throw _signedOut();
    var response = await _request(method, path, token, body);
    if (response.statusCode == 401 && _code(response) == 'token_expired') {
      token = await _session.refreshAfterTokenExpired(token);
      if (token == null) throw _signedOut();
      response = await _request(method, path, token, body);
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
  ) async {
    final request = http.Request(method, Uri.parse('$origin$path'))
      ..headers['Authorization'] = 'Bearer $token';
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
  return AccountApi(
    origin: origin,
    client: ref.watch(authHttpClientProvider),
    session: ref.watch(tokenSessionControllerProvider(origin)),
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

/// This account's signed-in devices. Invalidate to reload.
final accountDevicesProvider = FutureProvider.autoDispose
    .family<List<AccountDevice>, String>(
      (ref, origin) => ref.watch(accountApiProvider(origin)).listDevices(),
      retry: _noRetry,
    );
