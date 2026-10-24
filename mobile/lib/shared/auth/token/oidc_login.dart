import 'dart:math';

import '../../app_update/app_update.dart';
import '../../device_key/device_key.dart';
import '../../l10n/l10n.dart';
import 'auth_api.dart';
import 'google_account_picker.dart';
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
///
/// With [accountPicker], a Google login first offers the Google accounts
/// already on the phone; the browser opens only when the phone cannot show
/// them or the relay does not offer app sign-in.
Future<LoginGrant> runOidcLogin({
  required AuthApi api,
  required WebAuthLauncher launcher,
  GoogleAccountPicker? accountPicker,
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
    accountPicker: accountPicker,
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
  required GoogleAccountPicker? accountPicker,
  required String provider,
  required String? deviceName,
  required String? installId,
  required String identityMode,
  required String callbackScheme,
  required Random? random,
}) async {
  // The device key `start` binds must be the one proven at `complete`.
  final device = await api.deviceKeyInfo();
  // Learn the relay clock while the user is in the browser or the picker.
  final clock = device == null ? Future<void>.value() : api.syncClock();
  final redirectUri = Uri.parse('$callbackScheme://auth/cb');
  if (accountPicker != null && provider == 'google') {
    final grant = await _runAppSignIn(
      api: api,
      picker: accountPicker,
      provider: provider,
      redirectUri: redirectUri,
      deviceName: deviceName,
      installId: installId,
      identityMode: identityMode,
      device: device,
      clock: clock,
      random: random,
    );
    if (grant != null) return grant;
  }
  final pkce = PkcePair.generate(random);
  final kuzzClient = await appClientId();
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
      kuzzClient: kuzzClient,
    ),
    callbackUrlScheme: callbackScheme,
  );
  if (callback.scheme != redirectUri.scheme ||
      callback.authority != redirectUri.authority ||
      callback.path != redirectUri.path ||
      // iOS appends a bare `#` to the ASWebAuthenticationSession callback,
      // so only a fragment carrying data is rejected.
      callback.fragment.isNotEmpty) {
    throw OidcLoginException(
      appL10n.tokenSessionOidcUnexpectedCallback,
      code: 'invalid_callback',
    );
  }
  // Do not let a repeated query parameter select an arbitrary state/code.
  if ([
    'state',
    'code',
    'error',
  ].any((key) => (callback.queryParametersAll[key]?.length ?? 0) > 1)) {
    throw OidcLoginException(
      appL10n.tokenSessionOidcAmbiguous,
      code: 'invalid_callback',
    );
  }
  final params = callback.queryParameters;
  // State first: an unsolicited callback (forged code *or* forged error)
  // must not be acted on at all.
  if (params['state'] != pkce.state) {
    throw OidcLoginException(
      appL10n.tokenSessionOidcMismatch,
      code: 'state_mismatch',
    );
  }
  final error = params['error'];
  if (error != null) {
    // Too old to sign in: show the full-screen update.
    if (error == updateRequiredCode) updateRequiredSignal.value++;
    throw OidcLoginException(_describeError(error), code: error);
  }
  final code = params['code'];
  if (code == null || code.isEmpty) {
    throw OidcLoginException(
      appL10n.tokenSessionOidcNoCode,
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

/// Sign in with the phone's own account picker. `null`: the picker or the
/// relay's app sign-in is not available, so the caller opens the browser.
Future<LoginGrant?> _runAppSignIn({
  required AuthApi api,
  required GoogleAccountPicker picker,
  required String provider,
  required Uri redirectUri,
  required String? deviceName,
  required String? installId,
  required String identityMode,
  required DeviceKeyInfo? device,
  required Future<void> clock,
  required Random? random,
}) async {
  final pkce = PkcePair.generate(random);
  final AppSignInStart start;
  try {
    start = await api.startAppSignIn(
      provider: provider,
      state: pkce.state,
      codeChallenge: pkce.challenge,
      redirectUri: redirectUri.toString(),
      deviceName: deviceName,
      installId: installId,
      identityMode: identityMode,
      devicePubkey: device?.publicKeyHex,
    );
  } on AuthApiException catch (error) {
    // A relay without app sign-in refuses `response=json`.
    if (error.statusCode == 400 || error.statusCode == 404) return null;
    rethrow;
  }
  final String idToken;
  try {
    idToken = await picker.idToken(
      clientId: start.clientId,
      nonce: start.nonce,
    );
  } on GoogleAccountUnavailableException {
    return null;
  }
  final code = await api.exchangeIdToken(
    provider: provider,
    state: pkce.state,
    idToken: idToken,
  );
  await clock;
  return api.completeLogin(
    loginCode: code,
    codeVerifier: pkce.verifier,
    device: device,
  );
}

String _describeError(String code) => switch (code) {
  'access_denied' => appL10n.tokenSessionOidcCancelled,
  'principal_disabled' => appL10n.tokenSessionOidcAccountDisabled,
  'login_failed' => appL10n.tokenSessionOidcNotVerified,
  _ => appL10n.tokenSessionOidcFailed(code),
};
