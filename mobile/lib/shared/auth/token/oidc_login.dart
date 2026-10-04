import 'dart:math';

import 'auth_api.dart';
import 'pkce.dart';
import 'web_auth_launcher.dart';

/// Custom URL scheme the relay redirects mobile logins to (relay default
/// `AUTH_MOBILE_REDIRECT_SCHEMES=xyz.block.buzz`).
const buzzMobileCallbackScheme = 'xyz.block.buzz';

/// The exact `redirect_uri` the relay accepts for `client=mobile`.
const buzzMobileRedirectUri = '$buzzMobileCallbackScheme://auth/cb';

/// The OIDC login did not produce tokens.
class OidcLoginException implements Exception {
  const OidcLoginException(this.message, {this.code});

  final String message;

  /// Relay `error` callback code (`access_denied`, `login_failed`,
  /// `server_error`, `principal_disabled`) or `state_mismatch` /
  /// `missing_code`.
  final String? code;

  @override
  String toString() => 'OidcLoginException($code): $message';
}

/// Run one PKCE login: open the provider in the system browser, validate
/// the callback, and exchange the login code.
///
/// Throws [WebAuthCancelledException] when the user dismisses the browser,
/// [OidcLoginException] for a rejected or forged callback, and
/// [AuthApiException] when the code exchange fails.
Future<LoginGrant> runOidcLogin({
  required AuthApi api,
  required WebAuthLauncher launcher,
  String provider = 'google',
  String? deviceName,
  Random? random,
}) async {
  final pkce = PkcePair.generate(random);
  final callback = await launcher.authenticate(
    url: api.startUri(
      provider: provider,
      state: pkce.state,
      codeChallenge: pkce.challenge,
      redirectUri: buzzMobileRedirectUri,
      deviceName: deviceName,
    ),
    callbackUrlScheme: buzzMobileCallbackScheme,
  );
  final params = callback.queryParameters;
  // State first: an unsolicited callback (forged code *or* forged error)
  // must not be acted on at all.
  if (params['state'] != pkce.state) {
    throw const OidcLoginException(
      'The sign-in response did not match this request. Try again.',
      code: 'state_mismatch',
    );
  }
  final error = params['error'];
  if (error != null) {
    throw OidcLoginException(_describeError(error), code: error);
  }
  final code = params['code'];
  if (code == null || code.isEmpty) {
    throw const OidcLoginException(
      'The sign-in response had no login code.',
      code: 'missing_code',
    );
  }
  return api.completeLogin(loginCode: code, codeVerifier: pkce.verifier);
}

String _describeError(String code) => switch (code) {
  'access_denied' => 'Sign-in was cancelled at the provider.',
  'principal_disabled' => 'This account has been disabled on this relay.',
  'login_failed' => 'The provider could not verify this account.',
  _ => 'Sign-in failed ($code). Try again.',
};
