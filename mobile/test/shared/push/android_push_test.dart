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
    var permission = 'notDetermined';
    PlatformException? tokenError;
    var token = 'tok-1';
    setUp(() {
      debugAndroidPushActiveOverride = true;
      debugResetKuzzPushRegistrationState();
      apnsDeviceToken.value = null;
      permission = 'notDetermined';
      tokenError = null;
      token = 'tok-1';
      calls.clear();
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(androidPushChannel, (call) async {
            calls.add(call.method);
            switch (call.method) {
              case 'permissionStatus':
                return permission;
              case 'requestPermission':
                // The person says no; Android remembers it was asked.
                permission = 'denied';
                return permission;
              case 'token':
                if (tokenError != null) throw tokenError!;
                return token;
            }
            return null;
          });
    });
    tearDown(() {
      debugAndroidPushActiveOverride = null;
      debugResetKuzzPushRegistrationState();
      apnsDeviceToken.value = null;
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
        final prompts = kuzzPushPermissionPrompts.value;
        await startKuzzPushRegistration();
        expect(calls, ['permissionStatus', 'requestPermission', 'token']);
        expect(apnsDeviceToken.value, 'tok-1');
        // The status is read again after the dialog.
        expect(kuzzPushPermissionPrompts.value, prompts + 1);
      },
      skip: !Env.pushGatewayConfigured,
    );

    test(
      'a retry reads the permission and never asks again',
      () async {
        tokenError = PlatformException(
          code: 'token',
          message: 'IOException',
          details: 'SERVICE_NOT_AVAILABLE',
        );
        await expectLater(startKuzzPushRegistration(), throwsStateError);
        expect(calls, ['permissionStatus', 'requestPermission', 'token']);
        calls.clear();
        await expectLater(startKuzzPushRegistration(), throwsStateError);
        expect(calls, ['permissionStatus', 'token']);

        // Also not when it was never answered but was shown in this run.
        permission = 'notDetermined';
        calls.clear();
        tokenError = null;
        await startKuzzPushRegistration();
        expect(calls, ['permissionStatus', 'token']);
      },
      skip: !Env.pushGatewayConfigured,
    );

    test('turning push on again may ask once more', () async {
      await startKuzzPushRegistration();
      calls.clear();
      noteKuzzPushToggled('community', enabled: true);
      await startKuzzPushRegistration();
      expect(calls, ['permissionStatus', 'requestPermission', 'token']);
      calls.clear();
      await startKuzzPushRegistration();
      expect(calls, ['permissionStatus', 'token']);
    }, skip: !Env.pushGatewayConfigured);

    test('a phone without Google Play services stops until push is turned on '
        'again', () async {
      tokenError = PlatformException(
        code: 'token',
        message: 'IOException',
        details: 'java.io.IOException: MISSING_INSTANCEID_SERVICE',
      );
      await expectLater(
        startKuzzPushRegistration(),
        throwsA(isA<KuzzPushUnavailableOnThisPhone>()),
      );
      expect(kuzzPushUnavailableOnThisPhone.value, isTrue);
      calls.clear();
      await expectLater(
        startKuzzPushRegistration(),
        throwsA(isA<KuzzPushUnavailableOnThisPhone>()),
      );
      expect(calls, isEmpty);

      tokenError = null;
      noteKuzzPushToggled('community', enabled: true);
      expect(kuzzPushUnavailableOnThisPhone.value, isFalse);
      await startKuzzPushRegistration();
      expect(apnsDeviceToken.value, 'tok-1');
    }, skip: !Env.pushGatewayConfigured);

    test(
      'a transient FCM error never makes push unavailable (R1)',
      () async {
        for (final details in [
          'java.io.IOException: SERVICE_NOT_AVAILABLE',
          'java.util.concurrent.TimeoutException',
          'java.io.IOException: AUTHENTICATION_FAILED',
        ]) {
          tokenError = PlatformException(
            code: 'token',
            message: 'IOException',
            details: details,
          );
          for (var attempt = 0; attempt < 10; attempt++) {
            await expectLater(startKuzzPushRegistration(), throwsStateError);
          }
          expect(kuzzPushUnavailableOnThisPhone.value, isFalse);
        }
        tokenError = null;
        await startKuzzPushRegistration();
        expect(apnsDeviceToken.value, 'tok-1');
      },
      skip: !Env.pushGatewayConfigured,
    );

    for (final (name, error) in [
      (
        'INVALID_SENDER',
        PlatformException(
          code: 'token',
          message: 'ExecutionException',
          details: 'java.io.IOException: INVALID_SENDER',
        ),
      ),
      ('not_configured', PlatformException(code: 'not_configured')),
    ]) {
      test(
        'a permanent FCM error ($name) makes push unavailable at once (R1)',
        () async {
          tokenError = error;
          await expectLater(
            startKuzzPushRegistration(),
            throwsA(isA<KuzzPushUnavailableOnThisPhone>()),
          );
          expect(kuzzPushUnavailableOnThisPhone.value, isTrue);
        },
        skip: !Env.pushGatewayConfigured,
      );
    }

    test(
      'a refused token makes push unavailable at once (R1)',
      () async {
        token = 'x' * 513;
        await expectLater(
          startKuzzPushRegistration(),
          throwsA(isA<KuzzPushUnavailableOnThisPhone>()),
        );
        expect(kuzzPushUnavailableOnThisPhone.value, isTrue);
        expect(apnsDeviceToken.value, isNull);
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
