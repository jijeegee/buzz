import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/shared/push/android_push.dart';
import 'package:kuzz/shared/push/push_bridge.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  test('without KUZZ_ANDROID_PUSH nothing reaches the plugin', () async {
    final calls = <String>[];
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(androidPushChannel, (call) async {
          calls.add(call.method);
          return true;
        });
    addTearDown(
      () => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(androidPushChannel, null),
    );
    debugDefaultTargetPlatformOverride = TargetPlatform.android;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);

    expect(kuzzAndroidPushConfigured, isFalse);
    expect(androidPushActive, isFalse);
    expect(await configureKuzzAndroidPush(), isFalse);
    expect(await readAndroidPushInboxRoot(), isNull);
    await setAndroidPushAgeRestricted(true);
    await syncPendingKuzzPushNotificationResponse();
    expect(calls, isEmpty);
  });

  test('a tapped notification becomes the pending link', () async {
    pendingPushNotificationLink.value = null;
    expect(
      await handleAndroidPushCall(
        const MethodCall('notificationOpened', {
          'eventId': 'e1',
          'communityId': 'c1',
          'channelId': 'r1',
        }),
      ),
      'handled',
    );
    final link = pendingPushNotificationLink.value!;
    expect(link.messageId, 'e1');
    expect(link.communityId, 'c1');
    expect(link.channelId, 'r1');
    expect(
      await handleAndroidPushCall(
        const MethodCall('notificationOpened', {'eventId': 'e2'}),
      ),
      'ignored',
    );
    pendingPushNotificationLink.value = null;
  });
}
