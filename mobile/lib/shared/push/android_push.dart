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

/// Lets tests take the Android push paths without the build defines.
@visibleForTesting
bool? debugAndroidPushActiveOverride;

/// Whether this run uses Android push.
bool get androidPushActive =>
    debugAndroidPushActiveOverride ??
    (kuzzAndroidPushConfigured &&
        !kIsWeb &&
        defaultTargetPlatform == TargetPlatform.android);

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

/// Notification permission: `authorized`, `denied` or `notDetermined`
/// (Android 13+ before the person was asked).
Future<String?> readAndroidNotificationPermission() =>
    androidPushChannel.invokeMethod<String>('permissionStatus');

/// Asks for POST_NOTIFICATIONS on Android 13+ when not yet decided; returns
/// the permission afterwards. Older Android grants it at install.
Future<String?> requestAndroidNotificationPermission() =>
    androidPushChannel.invokeMethod<String>('requestPermission');

/// Opens this app's notification settings.
Future<bool> openAndroidNotificationSettings() async =>
    await androidPushChannel.invokeMethod<bool>('openNotificationSettings') ??
    false;
