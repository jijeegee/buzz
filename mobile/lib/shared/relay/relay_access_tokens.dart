import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:http/http.dart' as http;

import '../auth/token/relay_origin.dart';
import '../auth/token/token_session.dart';
import '../auth/token/token_session_provider.dart';
import 'relay_provider.dart';

/// Access tokens for the active token-auth community (plan §3.3–§3.6).
///
/// Every relay transport (HTTP bridge, Blossom, WebSocket, huddle audio)
/// reads its bearer through this seam so a legacy nsec community — where
/// [relayAccessTokensProvider] is `null` — keeps its NIP-42/NIP-98 paths
/// untouched.
abstract interface class RelayAccessTokens {
  /// The cached token if it is not near expiry; synchronous, for headers
  /// that cannot await (image loaders).
  String? get current;

  /// A usable token, rotating when needed; `null` when the session cannot
  /// produce one (signed out, backing off, stalled).
  Future<String?> fresh();

  /// The relay rejected [rejected] as `token_expired`: rotate once unless
  /// the session already moved past it.
  Future<String?> afterExpired(String rejected);

  /// Called with every newly issued token (sign-in and each rotation).
  void Function() addTokenListener(void Function(String token) listener);

  /// Called on every session status change.
  void Function() addStateListener(
    void Function(TokenSessionState state) listener,
  );
}

/// [RelayAccessTokens] backed by the origin's [TokenSessionController].
class ControllerRelayAccessTokens implements RelayAccessTokens {
  ControllerRelayAccessTokens(this._controller);

  final TokenSessionController _controller;

  @override
  String? get current => _controller.currentAccessToken;

  @override
  Future<String?> fresh() async {
    // Nothing may have started the restore yet (no UI watches the session
    // provider during a cold background start); restore() is single-flight.
    if (_controller.state.status == TokenSessionStatus.restoring) {
      await _controller.restore();
    }
    return _controller.ensureFreshAccessToken();
  }

  @override
  Future<String?> afterExpired(String rejected) =>
      _controller.refreshAfterTokenExpired(rejected);

  @override
  void Function() addTokenListener(void Function(String token) listener) =>
      _controller.addAccessTokenListener(listener);

  @override
  void Function() addStateListener(
    void Function(TokenSessionState state) listener,
  ) => _controller.addListener(listener);
}

/// The active community's token source, or `null` when there is none: a
/// legacy nsec community, or a token community whose relay URL has no
/// canonical origin (it cannot hold a session). Token-mode callers treat
/// `null` as [RelayTokenUnavailableException] (fail closed, never NIP-42).
final relayAccessTokensProvider = Provider<RelayAccessTokens?>((ref) {
  final config = ref.watch(relayConfigProvider);
  if (!config.tokenAuth) return null;
  final String origin;
  try {
    origin = normalizeRelayOrigin(config.baseUrl);
  } on FormatException catch (error) {
    debugPrint('[RelayAccessTokens] no canonical origin: $error');
    return null;
  }
  return ControllerRelayAccessTokens(
    ref.watch(tokenSessionControllerProvider(origin)),
  );
});

/// No access token is available: the token session is signed out, backing
/// off, or stalled. The session's own state carries the recovery path.
class RelayTokenUnavailableException implements Exception {
  const RelayTokenUnavailableException();

  @override
  String toString() =>
      'RelayTokenUnavailableException: not signed in to this relay';
}

/// Token AUTH rejections (relay WS and huddle audio) that one refresh may
/// cure (plan §3.4). A revoked or invalid access token is refreshed once
/// too: if the refresh token is dead as well, the session signs out and its
/// gate is the recovery path.
bool isRefreshableTokenRejection(String message) => const {
  'auth-required: token_expired',
  'auth-required: token_revoked',
  'auth-required: invalid_token',
}.contains(message);

/// Machine codes a relay uses to reject a bearer token.
const _tokenRejectionCodes = {
  'invalid_token',
  'token_expired',
  'token_revoked',
  'principal_disabled',
};

/// The relay's machine code from a token rejection body, or `null`.
///
/// The bridge/media surface answers `{"error":..., "code":"token_expired"}`;
/// the invites API answers `{"error":"token_expired"}` with no `code`.
String? relayAuthErrorCode(String body) {
  try {
    final decoded = jsonDecode(body);
    if (decoded is Map<String, dynamic>) {
      final code = decoded['code'];
      if (code is String) return code;
      final error = decoded['error'];
      return error is String && _tokenRejectionCodes.contains(error)
          ? error
          : null;
    }
  } on FormatException {
    return null;
  }
  return null;
}

/// Run [send] with `Authorization: Bearer <token>`. A 401 `token_expired`
/// rotates once and retries once; any other response (including a second
/// 401) is returned to the caller unchanged. Throws
/// [RelayTokenUnavailableException] when no token can be obtained.
Future<http.Response> sendWithBearer(
  RelayAccessTokens tokens,
  Future<http.Response> Function(String authorization) send,
) async {
  final token = await tokens.fresh();
  if (token == null) throw const RelayTokenUnavailableException();
  final response = await send('Bearer $token');
  if (response.statusCode != 401 ||
      relayAuthErrorCode(response.body) != 'token_expired') {
    return response;
  }
  final next = await tokens.afterExpired(token);
  if (next == null) throw const RelayTokenUnavailableException();
  return send('Bearer $next');
}

/// Send one relay HTTP request with the community's auth: a bearer token
/// (via [sendWithBearer]) on a token community, otherwise the NIP-98 header
/// produced by [nip98] (only called in legacy mode).
Future<http.Response> sendRelayAuthorized(
  RelayConfig config,
  RelayAccessTokens? tokens,
  Future<http.Response> Function(String authorization) send, {
  required String Function() nip98,
}) async {
  if (!config.tokenAuth) return send(nip98());
  if (tokens == null) throw const RelayTokenUnavailableException();
  return sendWithBearer(tokens, send);
}
