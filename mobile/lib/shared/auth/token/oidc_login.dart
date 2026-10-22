import 'dart:math';

import 'auth_api.dart';
import 'mobile_callback.dart';
import 'pkce.dart';
import 'web_auth_launcher.dart';

export 'mobile_callback.dart'
    show kuzzMobileCallbackScheme, kuzzMobileRedirectUri;

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
///
/// The device key proof at `complete` is stamped in relay time, learned
/// while the browser is open. If the relay still finds it stale (the login
/// code is then spent), the login starts over once with the corrected clock.
Future<LoginGrant> runOidcLogin({
  required AuthApi api,
  required WebAuthLauncher launcher,
  String provider = 'google',
  String? deviceName,
  String? installId,
  String identityMode = 'token',
  String callbackScheme = kuzzMobileCallbackScheme,
  Random? random,
}) async {
  Future<LoginGrant> attempt() => _runOidcLoginOnce(
    api: api,
    launcher: launcher,
    provider: provider,
    deviceName: deviceName,
    installId: installId,
    identityMode: identityMode,
    callbackScheme: callbackScheme,
    random: random,
  );
  try {
    return await attempt();
  } on AuthApiException catch (error) {
    if (error.code != 'device_proof_stale') rethrow;
    // The refusal carried the relay time; sign in again with it.
    return attempt();
  }
}

Future<LoginGrant> _runOidcLoginOnce({
  required AuthApi api,
  required WebAuthLauncher launcher,
  required String provider,
  required String? deviceName,
  required String? installId,
  required String identityMode,
  required String callbackScheme,
  required Random? random,
}) async {
  final pkce = PkcePair.generate(random);
  // The device key `start` binds must be the one proven at `complete`.
  final device = await api.deviceKeyInfo();
  // Learn the relay clock while the user is in the browser.
  final clock = device == null ? Future<void>.value() : api.syncClock();
  final redirectUri = Uri.parse('$callbackScheme://auth/cb');
  final callback = await launcher.authenticate(
    url: api.startUri(
      provider: provider,
      state: pkce.state,
      codeChallenge: pkce.challenge,
      redirectUri: redirectUri.toString(),
      deviceName: deviceName,
      installId: installId,
      identityMode: identityMode,
      devicePubkey: device?.publicKeyHex,
    ),
    callbackUrlScheme: callbackScheme,
  );
  if (callback.scheme != redirectUri.scheme ||
      callback.authority != redirectUri.authority ||
      callback.path != redirectUri.path ||
      // iOS appends a bare `#` to the ASWebAuthenticationSession callback,
      // so only a fragment carrying data is rejected.
      callback.fragment.isNotEmpty) {
    throw const OidcLoginException(
      'The sign-in response used an unexpected callback address. Try again.',
      code: 'invalid_callback',
    );
  }
  // Do not let a repeated query parameter select an arbitrary state/code.
  if ([
    'state',
    'code',
    'error',
  ].any((key) => (callback.queryParametersAll[key]?.length ?? 0) > 1)) {
    throw const OidcLoginException(
      'The sign-in response contained ambiguous parameters. Try again.',
      code: 'invalid_callback',
    );
  }
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
  await clock;
  return api.completeLogin(
    loginCode: code,
    codeVerifier: pkce.verifier,
    device: device,
  );
}

String _describeError(String code) => switch (code) {
  'access_denied' => 'Sign-in was cancelled at the provider.',
  'principal_disabled' => 'This account has been disabled on this relay.',
  'login_failed' => 'The provider could not verify this account.',
  _ => 'Sign-in failed ($code). Try again.',
};
