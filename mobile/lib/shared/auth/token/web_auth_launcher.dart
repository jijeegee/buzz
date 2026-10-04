import 'package:flutter/services.dart';
import 'package:flutter_web_auth_2/flutter_web_auth_2.dart';

/// The user closed the sign-in browser without finishing.
class WebAuthCancelledException implements Exception {
  const WebAuthCancelledException();

  @override
  String toString() => 'WebAuthCancelledException';
}

/// Opens the system auth browser and returns the callback URL.
///
/// Injectable so tests and widget tests never touch the platform channel.
abstract interface class WebAuthLauncher {
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
  Future<Uri> authenticate({
    required Uri url,
    required String callbackUrlScheme,
  }) async {
    try {
      final result = await FlutterWebAuth2.authenticate(
        url: url.toString(),
        callbackUrlScheme: callbackUrlScheme,
        // Do not share the browser's Google cookies with other apps' logins
        // and do not leave this login behind in the shared jar.
        options: const FlutterWebAuth2Options(preferEphemeral: true),
      );
      return Uri.parse(result);
    } on PlatformException catch (error) {
      if (error.code == 'CANCELED') throw const WebAuthCancelledException();
      rethrow;
    }
  }
}
