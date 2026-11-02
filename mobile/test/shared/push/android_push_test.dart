import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/testing.dart';
import 'package:http/http.dart' as http;
import 'package:kuzz/shared/push/android_push.dart';
import 'package:kuzz/shared/push/android_push_enrollment.dart';
import 'package:kuzz/shared/push/dev_push_lease.dart';
import 'package:kuzz/shared/push/push_bridge.dart';
import 'package:kuzz/shared/relay/relay_provider.dart' show Env;

import '../community/community_storage_test.dart' show FakeSecureStorage;

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

  group('Android push on', () {
    final calls = <String>[];
    setUp(() {
      debugAndroidPushActiveOverride = true;
      apnsDeviceToken.value = null;
      apnsRegistrationError.value = null;
      calls.clear();
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(androidPushChannel, (call) async {
            calls.add(call.method);
            return switch (call.method) {
              'requestPermission' => 'authorized',
              'token' => 'tok-1',
              _ => null,
            };
          });
    });
    tearDown(() {
      debugAndroidPushActiveOverride = null;
      apnsDeviceToken.value = null;
      apnsRegistrationError.value = null;
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(androidPushChannel, null);
    });

    test('the profile and transport are the Android ones', () {
      expect(kuzzPushAppProfile, 'kuzz-android');
      expect(kuzzPushTransport, 'fcm');
      debugAndroidPushActiveOverride = false;
      expect(kuzzPushAppProfile, 'kuzz-ios-dogfood');
      expect(kuzzPushTransport, 'apns');
    });

    test(
      'turning push on asks for the permission, then the token',
      () async {
        await startKuzzPushRegistration();
        expect(calls, ['requestPermission', 'token']);
        expect(apnsDeviceToken.value, 'tok-1');
      },
      skip: !Env.pushGatewayConfigured,
    );

    test('a refreshed token replaces the known one (S9)', () async {
      const refresh = MethodCall('tokenRefreshed', {'token': 'tok-2'});
      expect(await handleAndroidPushCall(refresh), 'ignored');
      expect(apnsDeviceToken.value, isNull);
      apnsDeviceToken.value = 'tok-1';
      expect(await handleAndroidPushCall(refresh), 'handled');
      expect(apnsDeviceToken.value, 'tok-2');
    });

    test('an oversized token is refused (N11)', () async {
      apnsDeviceToken.value = 'tok-1';
      await handleAndroidPushCall(
        MethodCall('tokenRefreshed', {'token': 'x' * 513}),
      );
      expect(apnsDeviceToken.value, 'tok-1');
      expect(apnsRegistrationError.value, 'FCM token refused');
    });

    test(
      'stored Android grants are read back as kuzz-android',
      () async {
        final store = AndroidPushEnrollmentStore(FakeSecureStorage());
        await store.saveGrant(
          AndroidPushGrantRecord(
            gatewayOrigin: Uri.parse(Env.pushGatewayUrl).origin,
            relayOrigin: 'wss://relay.example',
            relayPubkey: 'a' * 64,
            relayMetadataPubkey: null,
            installationHandle: '22222222-2222-4222-8222-222222222222',
            keyAlias: 'kuzz.push.enroll.1',
            installationId: '0' * 32,
            endpointGrant: 'grant',
            endpointHash: 'f' * 64,
            endpointEpoch: 1,
            generation: 1,
            expiresAt: 1900000000,
          ),
        );
        final previous = debugAndroidPushEnrollmentFactory;
        debugAndroidPushEnrollmentFactory = (client, gatewayUrl) =>
            AndroidPushEnrollment(
              gatewayUrl: gatewayUrl,
              client: MockClient((_) async => http.Response('', 500)),
              store: store,
            );
        addTearDown(() => debugAndroidPushEnrollmentFactory = previous);
        final grants = await readKuzzPushEndpointGrants();
        expect(grants.single.appProfile, 'kuzz-android');
        expect(grants.single.endpointGrant, 'grant');
        await expectLater(
          enrollKuzzPush('wss://relay.example', Env.pushGatewayUrl),
          throwsStateError,
        );
      },
      skip: !Env.pushGatewayConfigured,
    );
  });
}
