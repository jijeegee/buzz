import 'dart:convert';

import 'package:http/http.dart' as http;

import 'device_key.dart';
import 'device_proof.dart';

/// HTTP client that adds [deviceProofHeader] to account calls (`/auth/*`):
///
/// - with `Authorization: Bearer <access>`: purpose `auth`, bound to the
///   method and URL, the token and the body;
/// - `POST /auth/refresh` with `{"refresh": ...}`: purpose `refresh`, bound
///   the same way to the refresh token.
///
/// Other requests pass through unchanged, as does everything when this
/// phone has no device key. Every `/auth/*` response teaches the relay's
/// clock ([serverTimeHeader], per origin). A proof refused because the
/// phone clock is off (`401` with a relay time far from the one assumed) is
/// re-signed in relay time and sent once more.
///
/// A refresh is never sent without its proof when this phone has a key:
/// if signing fails it throws [DeviceProofUnavailableException] (a
/// transient failure) rather than have the relay end the session.
class DeviceProofClient extends http.BaseClient {
  DeviceProofClient(this._inner, {DeviceKey? deviceKey})
    : _deviceKey = deviceKey;

  final http.Client _inner;
  final DeviceKey? _deviceKey;

  DeviceKey get _device => _deviceKey ?? DeviceKey.instance;

  @override
  Future<http.StreamedResponse> send(http.BaseRequest request) async {
    if (!_isAccountCall(request.url)) return _inner.send(request);
    final origin = request.url.origin;
    if (request is! http.Request) {
      return _noteTime(origin, await _inner.send(request));
    }
    final proven = await _withProof(request);
    if (proven == null) {
      if (_isRefresh(request) && await _device.info() != null) {
        throw const DeviceProofUnavailableException();
      }
      return _noteTime(origin, await _inner.send(request));
    }
    final response = await _inner.send(proven);
    final clockMoved = noteServerTime(
      origin,
      response.headers[serverTimeHeader],
    );
    if (response.statusCode != 401 || !clockMoved) return response;
    // Proof time was off: re-sign in relay time and try once more.
    final retry = await _withProof(request);
    if (retry == null) return response;
    await response.stream.drain<void>();
    return _noteTime(origin, await _inner.send(retry));
  }

  static http.StreamedResponse _noteTime(
    String origin,
    http.StreamedResponse response,
  ) {
    noteServerTime(origin, response.headers[serverTimeHeader]);
    return response;
  }

  @override
  void close() => _inner.close();

  static bool _isAccountCall(Uri url) =>
      (url.scheme == 'https' || url.scheme == 'http') &&
      url.path.startsWith('/auth/');

  static bool _isRefresh(http.Request request) =>
      request.url.path == '/auth/refresh' && request.bodyBytes.isNotEmpty;

  /// A copy of [request] carrying a fresh proof, or `null` when none applies.
  Future<http.Request?> _withProof(http.Request request) async {
    final body = request.bodyBytes;
    final String purpose;
    final String token;
    final bearer = _bearer(request.headers);
    if (bearer != null) {
      purpose = proofPurposeAuth;
      token = bearer;
    } else if (_isRefresh(request)) {
      final refresh = _refreshToken(body);
      if (refresh == null) return null;
      purpose = proofPurposeRefresh;
      token = refresh;
    } else {
      return null;
    }
    final proof = await signHeaderProof(
      device: _device,
      purpose: purpose,
      origin: request.url.origin,
      binding: requestProofBinding(request.method, request.url),
      tokenHash: sha256Bytes(utf8.encode(token)),
      bodyHash: body.isEmpty ? null : sha256Bytes(body),
    );
    if (proof == null) return null;
    return _copy(request)..headers[deviceProofHeader] = proof;
  }

  static String? _bearer(Map<String, String> headers) {
    for (final entry in headers.entries) {
      if (entry.key.toLowerCase() != 'authorization') continue;
      final value = entry.value.trim();
      if (value.length > 7 &&
          value.substring(0, 7).toLowerCase() == 'bearer ') {
        return value.substring(7).trim();
      }
    }
    return null;
  }

  static String? _refreshToken(List<int> body) {
    try {
      final decoded = jsonDecode(utf8.decode(body));
      final refresh = decoded is Map ? decoded['refresh'] : null;
      return refresh is String && refresh.isNotEmpty ? refresh : null;
    } on FormatException {
      return null;
    }
  }

  static http.Request _copy(http.Request request) {
    final copy = http.Request(request.method, request.url)
      ..followRedirects = request.followRedirects
      ..maxRedirects = request.maxRedirects
      ..persistentConnection = request.persistentConnection
      ..headers.addAll(request.headers);
    final bytes = request.bodyBytes;
    if (bytes.isNotEmpty) copy.bodyBytes = bytes;
    return copy;
  }
}

/// This phone has a device key but could not sign the proof a request
/// needs (the secure hardware was busy or failed). Transient: try later.
class DeviceProofUnavailableException implements Exception {
  const DeviceProofUnavailableException();

  @override
  String toString() => 'DeviceProofUnavailableException';
}
