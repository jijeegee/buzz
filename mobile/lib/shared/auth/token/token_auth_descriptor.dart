import 'dart:convert';

import 'package:http/http.dart' as http;

/// The relay's `buzz_token_auth` NIP-11 descriptor (plan §3.2).
///
/// Present only on relays with centralized token auth enabled; such relays
/// accept `Authorization: Bearer` and `["AUTH", {"token": ...}]` and offer
/// OIDC sign-in.
class TokenAuthDescriptor {
  const TokenAuthDescriptor({required this.oidcProviders});

  /// Provider names accepted by `/auth/oidc/{provider}/start`.
  final List<String> oidcProviders;

  /// Whether "Sign in with Google" can be offered.
  bool get supportsGoogle => oidcProviders.contains('google');
}

/// Parse the descriptor from a decoded NIP-11 document.
///
/// Returns `null` when the relay does not advertise bearer token auth
/// (`buzz_token_auth.bearer` must be exactly `true`, matching web).
TokenAuthDescriptor? parseTokenAuthDescriptor(Object? nip11) {
  if (nip11 is! Map) return null;
  final raw = nip11['buzz_token_auth'];
  if (raw is! Map || raw['bearer'] != true) return null;
  final providers = raw['oidc_providers'];
  return TokenAuthDescriptor(
    oidcProviders: providers is List
        ? List.unmodifiable(providers.whereType<String>())
        : const [],
  );
}

/// The relay could not be asked whether it uses token auth.
///
/// Distinct from "the relay is legacy" (`null` from
/// [fetchTokenAuthDescriptor]) so callers never mistake an outage for a
/// legacy relay.
class TokenAuthDetectionException implements Exception {
  const TokenAuthDetectionException(this.message);

  final String message;

  @override
  String toString() => 'TokenAuthDetectionException: $message';
}

/// Fetch the NIP-11 document at [origin] and parse its token-auth
/// descriptor.
///
/// Returns `null` only when the relay answered and does not advertise token
/// auth. Network failures, non-200 responses and malformed JSON throw
/// [TokenAuthDetectionException].
Future<TokenAuthDescriptor?> fetchTokenAuthDescriptor(
  String origin,
  http.Client client, {
  Duration timeout = const Duration(seconds: 10),
}) async {
  final http.Response response;
  try {
    response = await client
        .get(
          Uri.parse('$origin/'),
          headers: const {'Accept': 'application/nostr+json'},
        )
        .timeout(timeout);
  } catch (error) {
    throw TokenAuthDetectionException('Could not reach the relay: $error');
  }
  if (response.statusCode != 200) {
    throw TokenAuthDetectionException(
      'Relay info request failed (HTTP ${response.statusCode})',
    );
  }
  final Object? json;
  try {
    json = jsonDecode(response.body);
  } on FormatException {
    throw const TokenAuthDetectionException('Relay info is not valid JSON');
  }
  if (json is! Map) {
    throw const TokenAuthDetectionException('Relay info is not a JSON object');
  }
  return parseTokenAuthDescriptor(json);
}
