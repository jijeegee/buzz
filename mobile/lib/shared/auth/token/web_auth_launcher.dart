import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter_web_auth_2/flutter_web_auth_2.dart';

import 'mobile_callback.dart';

/// The user closed the sign-in browser without finishing.
class WebAuthCancelledException implements Exception {
  const WebAuthCancelledException();

  @override
  String toString() => 'WebAuthCancelledException';
}

/// Another account/controller already owns the system authentication browser.
class WebAuthBusyException implements Exception {
  const WebAuthBusyException();
}

/// Opens the system auth browser and returns the callback URL.
///
/// Injectable so tests and widget tests never touch the platform channel.
abstract interface class WebAuthLauncher {
  /// Callback scheme registered by the installed application.
  Future<String> callbackScheme();

  /// Open [url] and resolve with the first navigation to
  /// `callbackUrlScheme://...`. Throws [WebAuthCancelledException] when the
  /// user dismisses the browser.
  Future<Uri> authenticate({
    required Uri url,
    required String callbackUrlScheme,
  });
}

/// [WebAuthLauncher] backed by `flutter_web_auth_2`
/// (ASWebAuthenticationSession on iOS, Auth Tab / Custom Tab on Android).
class FlutterWebAuth2Launcher implements WebAuthLauncher {
  const FlutterWebAuth2Launcher();

  @override
  Future<String> callbackScheme() => installedMobileCallbackScheme();

  // The platform plugin has one pending callback per scheme. A second call
  // overwrites it on Android and strands the first Future. Keep the guard
  // across launcher instances as well as token/key-backup/origin controllers.
  static bool _authenticating = false;

  @override
  Future<Uri> authenticate({
    required Uri url,
    required String callbackUrlScheme,
  }) async {
    if (_authenticating) throw const WebAuthBusyException();
    _authenticating = true;
    try {
      final result = await FlutterWebAuth2.authenticate(
        url: url.toString(),
        callbackUrlScheme: callbackUrlScheme,
        // Do not share the browser's Google cookies with other apps' logins
        // and do not leave this login behind in the shared jar.
        options: const FlutterWebAuth2Options(preferEphemeral: true),
      );
      if (defaultTargetPlatform == TargetPlatform.android) {
        try {
          await const MethodChannel(
            'buzz/auth_browser',
          ).invokeMethod<void>('returnToApp');
        } on PlatformException {
          // Window recovery must not discard an already received callback.
        } on MissingPluginException {
          // Older host builds can still complete sign-in after manual return.
        }
      }
      return Uri.parse(result);
    } on PlatformException catch (error) {
      if (error.code == 'CANCELED') throw const WebAuthCancelledException();
      rethrow;
    } finally {
      _authenticating = false;
    }
  }
}
