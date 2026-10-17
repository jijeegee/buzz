import 'dart:async';
import 'dart:io';

import 'package:http/http.dart' as http;

import '../../shared/auth/account/account_api.dart';

/// Shown when the request never got an answer.
const couldNotConnectMessage = 'Couldn’t connect. Try again.';

/// A fixed, readable sentence for [error], never the raw server or socket
/// text. [fallback] covers anything without a more specific meaning, and
/// should say what failed ("Couldn’t block Ann. Try again.").
String friendlyError(Object error, {required String fallback}) {
  if (error is AccountApiException) {
    switch ((error.statusCode, error.code)) {
      case (_, 'block_limit'):
        return 'You’ve blocked the most people you can.';
      case (_, 'username_taken'):
        return 'That ID is taken';
      case (429, _):
        return 'Too many tries. Wait a minute and try again.';
      case (403, _):
        return 'You don’t have permission to do that.';
      case (401, _):
        return 'Your sign-in expired. Sign in again.';
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
