import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

import 'web_auth_launcher.dart';

/// The phone cannot offer its own Google account picker (no Google account,
/// no Play services, or the app is not registered with Google). The login
/// falls back to the browser.
class GoogleAccountUnavailableException implements Exception {
  const GoogleAccountUnavailableException(this.reason);

  final String reason;

  @override
  String toString() => 'GoogleAccountUnavailableException($reason)';
}

/// Signs in with a Google account already on the phone, without a browser.
///
/// Injectable so tests never touch the platform channel.
abstract interface class GoogleAccountPicker {
  /// Show the phone's Google account picker and return the id_token for
  /// [clientId] carrying [nonce]. Throws [WebAuthCancelledException] when the
  /// user closes the picker and [GoogleAccountUnavailableException] when the
  /// phone cannot show it.
  Future<String> idToken({required String clientId, required String nonce});
}

/// The account picker for this platform, or `null` where only the browser
/// sign-in exists.
GoogleAccountPicker? platformGoogleAccountPicker() =>
    defaultTargetPlatform == TargetPlatform.android
    ? const AndroidGoogleAccountPicker()
    : null;

/// [GoogleAccountPicker] backed by Android Credential Manager
/// (`MainActivity`'s `kuzz/google_account` channel).
class AndroidGoogleAccountPicker implements GoogleAccountPicker {
  const AndroidGoogleAccountPicker();

  static const _channel = MethodChannel('kuzz/google_account');

  @override
  Future<String> idToken({
    required String clientId,
    required String nonce,
  }) async {
    final String? token;
    try {
      token = await _channel.invokeMethod<String>('idToken', {
        'clientId': clientId,
        'nonce': nonce,
      });
    } on PlatformException catch (error) {
      if (error.code == 'CANCELED') throw const WebAuthCancelledException();
      throw GoogleAccountUnavailableException(error.code);
    } on MissingPluginException {
      throw const GoogleAccountUnavailableException('missing_plugin');
    }
    if (token == null || token.isEmpty) {
      throw const GoogleAccountUnavailableException('empty_token');
    }
    return token;
  }
}
