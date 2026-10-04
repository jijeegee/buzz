import 'dart:convert';
import 'dart:math';

import 'package:crypto/crypto.dart';

/// Base64url without padding (RFC 7636 §3).
String base64UrlNoPadding(List<int> bytes) =>
    base64Url.encode(bytes).replaceAll('=', '');

/// [byteLength] random bytes as unpadded base64url.
String randomUrlSafe(int byteLength, [Random? random]) {
  final source = random ?? Random.secure();
  return base64UrlNoPadding(
    List<int>.generate(byteLength, (_) => source.nextInt(256)),
  );
}

/// S256 challenge for [verifier]: `base64url(sha256(verifier))`, 43 chars.
String pkceChallenge(String verifier) =>
    base64UrlNoPadding(sha256.convert(ascii.encode(verifier)).bytes);

/// A PKCE verifier/challenge pair plus the CSRF `state` for one login.
class PkcePair {
  const PkcePair({
    required this.verifier,
    required this.challenge,
    required this.state,
  });

  /// Fresh 32-byte verifier (43 chars) and 24-byte state (32 chars), both
  /// within the relay's URL-safe length limits.
  factory PkcePair.generate([Random? random]) {
    final verifier = randomUrlSafe(32, random);
    return PkcePair(
      verifier: verifier,
      challenge: pkceChallenge(verifier),
      state: randomUrlSafe(24, random),
    );
  }

  final String verifier;
  final String challenge;
  final String state;
}
