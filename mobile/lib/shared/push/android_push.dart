import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:kuzz_android_push/kuzz_android_push.dart';

import 'push_fcm_background.dart' show androidPushAgeMarkerPath;

/// Android push (stage B A3/A4) is built in only with
/// `--dart-define=KUZZ_ANDROID_PUSH=true` and the app's four Firebase
/// identifiers. Without all five, Android behaves as before: no Firebase,
/// no permission prompt, no push calls.
const kuzzAndroidPushRequested = bool.fromEnvironment('KUZZ_ANDROID_PUSH');
const _firebaseApiKey = String.fromEnvironment('KUZZ_FIREBASE_API_KEY');
const _firebaseAppId = String.fromEnvironment('KUZZ_FIREBASE_APP_ID');
const _firebaseSenderId = String.fromEnvironment('KUZZ_FIREBASE_SENDER_ID');
const _firebaseProjectId = String.fromEnvironment('KUZZ_FIREBASE_PROJECT_ID');

const kuzzAndroidPushConfigured =
    kuzzAndroidPushRequested &&
    _firebaseApiKey != '' &&
    _firebaseAppId != '' &&
    _firebaseSenderId != '' &&
    _firebaseProjectId != '';

/// Whether this run uses Android push. Tests override the platform.
bool get androidPushActive =>
    kuzzAndroidPushConfigured &&
    !kIsWeb &&
    defaultTargetPlatform == TargetPlatform.android;

const androidPushChannel = MethodChannel(kuzzAndroidPushChannel);

/// Starts Firebase from the build's identifiers (no google-services.json,
/// no Google Services Gradle plugin). Only called when [androidPushActive].
Future<bool> configureKuzzAndroidPush() async {
  if (!androidPushActive) return false;
  try {
    return await androidPushChannel.invokeMethod<bool>('configure', {
          'apiKey': _firebaseApiKey,
          'appId': _firebaseAppId,
          'senderId': _firebaseSenderId,
          'projectId': _firebaseProjectId,
        }) ??
        false;
  } on MissingPluginException {
    return false;
  } on PlatformException catch (error) {
    debugPrint('[AndroidPush] configure failed: ${error.code}');
    return false;
  }
}

/// `push-inbox/` in the app's no-backup files, or null without Android push.
Future<String?> readAndroidPushInboxRoot() async {
  if (!androidPushActive) return null;
  try {
    return await androidPushChannel.invokeMethod<String>('pushInboxRoot');
  } on MissingPluginException {
    return null;
  } on PlatformException catch (error) {
    debugPrint('[AndroidPush] inbox root unavailable: ${error.code}');
    return null;
  }
}

/// Marks or clears the age restriction the wake reads: while marked it
/// shows only the generic text and keeps nothing. Marking also removes
/// notifications already shown.
Future<void> setAndroidPushAgeRestricted(bool restricted) async {
  final root = await readAndroidPushInboxRoot();
  if (root == null) return;
  final marker = File(androidPushAgeMarkerPath(root));
  if (restricted) {
    await marker.parent.create(recursive: true);
    await marker.writeAsString('1', flush: true);
    await androidPushChannel.invokeMethod<void>('cancelAll');
  } else if (await marker.exists()) {
    await marker.delete();
  }
}
