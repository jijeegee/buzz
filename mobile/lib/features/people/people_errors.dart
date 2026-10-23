import 'dart:async';
import 'dart:io';

import 'package:http/http.dart' as http;

import '../../shared/auth/account/account_api.dart';
import '../../shared/l10n/l10n.dart';

/// Shown when the request never got an answer.
String get couldNotConnectMessage => appL10n.friendsErrorCouldNotConnect;

/// A fixed, readable sentence for [error], never the raw server or socket
/// text. [fallback] covers anything without a more specific meaning, and
/// should say what failed ("Couldn’t block Ann. Try again.").
String friendlyError(Object error, {required String fallback}) {
  if (error is AccountApiException) {
    switch ((error.statusCode, error.code)) {
      case (_, 'block_limit'):
        return appL10n.friendsErrorBlockLimit;
      case (_, 'username_taken'):
        return appL10n.friendsErrorIdTaken;
      case (429, _):
        return appL10n.friendsErrorTooManyTries;
      case (403, _):
        return appL10n.friendsErrorNoPermission;
      case (401, _):
        return appL10n.friendsErrorSignInExpired;
      case (null, _):
        return couldNotConnectMessage;
      case (final int status, _) when status >= 500:
        return couldNotConnectMessage;
    }
    return fallback;
  }
  if (error is SocketException ||
      error is TimeoutException ||
      error is HttpException ||
      error is http.ClientException) {
    return couldNotConnectMessage;
  }
  final text = '$error'.toLowerCase();
  if (text.contains('timed out') ||
      text.contains('timeout') ||
      text.contains('not connected') ||
      text.contains('disconnected') ||
      text.contains('socket')) {
    return couldNotConnectMessage;
  }
  return fallback;
}
