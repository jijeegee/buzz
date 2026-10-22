import 'dart:convert';

import 'package:http/http.dart' as http;

import '../../device_key/device_key.dart';
import '../../device_key/device_proof.dart';

/// Whether retrying the same request can succeed.
enum AuthFailureKind {
  /// The relay rejected the credential (400/401/403/404; for a refresh only
  /// 400/401, see [AuthApi.refresh]). Retrying with the same input will fail
  /// again; for a refresh this means the session is gone.
  terminal,

  /// Network failure, timeout, 429, 5xx or a malformed success body. The
  /// same request may succeed later.
  transient,
}

/// A failed `/auth/*` request.
class AuthApiException implements Exception {
  const AuthApiException(this.kind, this.message, {this.statusCode, this.code});

  final AuthFailureKind kind;

  /// Human-readable reason (relay `error` field when present).
  final String message;

  /// HTTP status, or `null` when no response arrived.
  final int? statusCode;

  /// Relay machine code (`invalid_grant`, `token_expired`,
  /// `refresh_reused`, ...).
  final String? code;

  bool get isTerminal => kind == AuthFailureKind.terminal;

  @override
  String toString() =>
      'AuthApiException(${kind.name}, $statusCode, $code): $message';
}

/// Tokens issued by `POST /auth/oidc/complete`.
class LoginGrant {
  const LoginGrant({
    required this.principalId,
    required this.deviceId,
    required this.accessToken,
    required this.refreshToken,
    required this.expiresIn,
    this.identityMode = 'token',
  });

  final String principalId;
  final String deviceId;
  final String accessToken;
  final String refreshToken;
  final Duration expiresIn;
  final String identityMode;
}

/// Tokens issued by `POST /auth/refresh` (the old refresh is consumed).
class RotatedTokens {
  const RotatedTokens({
    required this.accessToken,
    required this.refreshToken,
    required this.expiresIn,
  });

  final String accessToken;
  final String refreshToken;
  final Duration expiresIn;
}

/// The relay's token-auth HTTP endpoints for one origin (plan §3.2–3.6).
class AuthApi {
  AuthApi({
    required this.origin,
    required http.Client client,
    this.timeout = const Duration(seconds: 15),
    this.deviceKey,
  }) : _client = client;

  /// Canonical relay origin (see `normalizeRelayOrigin`).
  final String origin;
  final http.Client _client;
  final Duration timeout;

  /// This install's device key, presented and proven at sign-in; `null`
  /// signs in without one (older relays ignore the extra fields).
  final DeviceKey? deviceKey;

  /// The device key to bind at sign-in, or `null` when there is none.
  Future<DeviceKeyInfo?> deviceKeyInfo() async => await deviceKey?.info();

  /// Learn the relay clock before a proof must be stamped in it (a fresh
  /// install knows none). Any `/auth/*` answer carries
  /// [serverTimeHeader]; an unauthenticated `GET /auth/me` is the cheapest.
  /// Failures are ignored: the proof then uses phone time.
  Future<void> syncClock() async {
    if (hasServerClock(origin)) return;
    try {
      final response = await _client
          .get(Uri.parse('$origin/auth/me'))
          .timeout(timeout);
      noteServerTime(origin, response.headers[serverTimeHeader]);
    } catch (_) {
      // Offline or slow: sign in phone time.
    }
  }

  /// The browser URL that starts an OIDC login for a mobile client.
  Uri startUri({
    required String provider,
    required String state,
    required String codeChallenge,
    required String redirectUri,
    String? deviceName,
    String? installId,
    String identityMode = 'token',
    String? devicePubkey,
  }) {
    final name = deviceName?.trim();
    return Uri.parse(
      '$origin/auth/oidc/${Uri.encodeComponent(provider)}/start',
    ).replace(
      queryParameters: {
        'state': state,
        'code_challenge': codeChallenge,
        'client': 'mobile',
        'identity_mode': identityMode,
        'redirect_uri': redirectUri,
        if (name != null && name.isNotEmpty) 'device_name': name,
        if (installId != null && installId.isNotEmpty) 'install_id': installId,
        'device_pubkey': ?devicePubkey,
      },
    );
  }

  /// Exchange a one-time login code (`bzl_...`) and its PKCE verifier.
  ///
  /// The relay consumes the code on the first attempt, so a failure here is
  /// never retried with the same code.
  ///
  /// With [device] (the key `start` was given), the body also carries
  /// `device_pubkey`, `device_key_kind` and `device_proof`: a `bind` proof
  /// over `login:<sha256(code)>`. If signing fails the key fields are left
  /// out, and the relay records the device without a key.
  Future<LoginGrant> completeLogin({
    required String loginCode,
    required String codeVerifier,
    DeviceKeyInfo? device,
  }) async {
    final key = deviceKey;
    final proof = device == null || key == null
        ? null
        : await signHeaderProof(
            device: key,
            origin: origin,
            purpose: proofPurposeBind,
            binding: loginProofBinding(loginCode),
          );
    final json = await _post('/auth/oidc/complete', {
      'login_code': loginCode,
      'code_verifier': codeVerifier,
      if (device != null && proof != null) ...{
        'device_pubkey': device.publicKeyHex,
        'device_key_kind': device.kind,
        'device_proof': proof,
      },
    });
    return LoginGrant(
      principalId: _string(json, 'principal_id'),
      deviceId: _string(json, 'device_id'),
      accessToken: _string(json, 'access'),
      refreshToken: _string(json, 'refresh'),
      expiresIn: _expiresIn(json),
      identityMode: json['identity_mode'] is String
          ? json['identity_mode'] as String
          : 'token',
    );
  }

  /// Rotate [refreshToken]. The relay consumes it; persist the returned
  /// refresh before using the returned access token.
  ///
  /// Only a 400/401 here is a credential verdict, except a 401 refusing the
  /// device proof (`device_proof_required`/`device_proof_stale`), which is
  /// transient like a failure to sign the proof at all. The refresh handler never
  /// answers 403/404; those come from the relay's unrouted fallback when
  /// `AUTH_TOKEN_ENABLED` is off (e.g. a rollback or misrouted proxy), so they
  /// are transient: the session stalls and is kept rather than wiped.
  Future<RotatedTokens> refresh(String refreshToken) async {
    final Map<String, Object?> json;
    try {
      json = await _post('/auth/refresh', {'refresh': refreshToken});
    } on AuthApiException catch (error) {
      final status = error.statusCode;
      // A refused device proof says nothing about the session itself (the
      // phone's key or clock was off): keep the session and retry later.
      final proofRefused =
          status == 401 && _transientProofCodes.contains(error.code);
      if (status != 403 && status != 404 && !proofRefused) rethrow;
      throw AuthApiException(
        AuthFailureKind.transient,
        error.message,
        statusCode: status,
        code: error.code,
      );
    }
    return RotatedTokens(
      accessToken: _string(json, 'access'),
      refreshToken: _string(json, 'refresh'),
      expiresIn: _expiresIn(json),
    );
  }

  /// Revoke this device's session (`POST /auth/logout`).
  Future<void> logout(String accessToken) async {
    await _post(
      '/auth/logout',
      const <String, Object?>{},
      accessToken: accessToken,
      expectBody: false,
    );
  }

  Future<Map<String, Object?>> _post(
    String path,
    Map<String, Object?> body, {
    String? accessToken,
    bool expectBody = true,
  }) async {
    final http.Response response;
    try {
      response = await _client
          .post(
            Uri.parse('$origin$path'),
            headers: {
              'Content-Type': 'application/json',
              if (accessToken != null) 'Authorization': 'Bearer $accessToken',
            },
            body: jsonEncode(body),
          )
          .timeout(timeout);
    } catch (error) {
      throw AuthApiException(
        AuthFailureKind.transient,
        'Could not reach the relay: $error',
      );
    }
    noteServerTime(origin, response.headers[serverTimeHeader]);
    final status = response.statusCode;
    if (status < 200 || status >= 300) {
      final error = _decodeObject(response.body);
      final code = error?['code'];
      final message = error?['error'];
      throw AuthApiException(
        _terminalStatuses.contains(status)
            ? AuthFailureKind.terminal
            : AuthFailureKind.transient,
        message is String && message.isNotEmpty ? message : 'HTTP $status',
        statusCode: status,
        code: code is String ? code : null,
      );
    }
    if (!expectBody) return const {};
    final json = _decodeObject(response.body);
    if (json == null) {
      throw AuthApiException(
        AuthFailureKind.transient,
        'Malformed relay response',
        statusCode: status,
      );
    }
    return json;
  }

  /// Statuses meaning "this credential/input is rejected": retrying the
  /// same request cannot succeed. 408/429/5xx stay transient.
  static const _terminalStatuses = {400, 401, 403, 404};

  /// Device-proof refusals that a refresh treats as transient.
  static const _transientProofCodes = {
    'device_proof_required',
    'device_proof_stale',
  };

  static Map<String, Object?>? _decodeObject(String body) {
    try {
      final decoded = jsonDecode(body);
      return decoded is Map<String, Object?> ? decoded : null;
    } on FormatException {
      return null;
    }
  }

  static String _string(Map<String, Object?> json, String key) {
    final value = json[key];
    if (value is String && value.isNotEmpty) return value;
    throw AuthApiException(
      AuthFailureKind.transient,
      'Malformed relay response: missing $key',
    );
  }

  static Duration _expiresIn(Map<String, Object?> json) {
    final value = json['expires_in'];
    final seconds = value is num && value > 0 ? value.toInt() : 3600;
    return Duration(seconds: seconds);
  }
}
