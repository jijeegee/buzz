import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:kuzz/features/settings/settings_page.dart';
import 'package:kuzz/shared/auth/auth_provider.dart';
import 'package:kuzz/shared/community/community.dart';
import 'package:kuzz/shared/community/community_provider.dart';
import 'package:kuzz/shared/community/community_storage.dart';
import 'package:kuzz/shared/push/android_push.dart';
import 'package:kuzz/shared/push/dev_push_lease.dart';
import 'package:kuzz/shared/push/push_bridge.dart';
import 'package:kuzz/shared/push/push_relay_capability_provider.dart';
import 'package:kuzz/shared/push/push_subscription.dart';
import 'package:kuzz/shared/push/push_subscription_provider.dart';
import 'package:kuzz/shared/relay/app_lifecycle_provider.dart';
import 'package:kuzz/shared/theme/theme.dart';
import 'package:nostr/nostr.dart' as nostr;
import 'package:package_info_plus/package_info_plus.dart';
import 'package:shared_preferences/shared_preferences.dart';

import '../../shared/community/community_storage_test.dart';

/// Stage B A3.4: the notification permission on Android.
void main() {
  tearDown(() => debugAndroidPushActiveOverride = null);

  Future<void> pumpSettings(
    WidgetTester tester, {
    required KuzzPushAuthorizationStatus status,
    bool buildSupported = true,
  }) async {
    tester.view.physicalSize = const Size(430, 1800);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    PackageInfo.setMockInitialValues(
      appName: 'Kuzz',
      packageName: 'kuzz',
      version: '1.0.0',
      buildNumber: '1',
      buildSignature: '',
    );
    SharedPreferences.setMockInitialValues({});
    final prefs = await SharedPreferences.getInstance();
    final storage = CommunityStorage(secure: FakeSecureStorage());
    final subscriptions = [
      KuzzPushSubscription(
        filter: KuzzPushFilter(kinds: const [9], pTags: ['a' * 64]),
        notificationClass: 'default',
      ),
    ];
    final community =
        Community.create(
          name: 'Team',
          relayUrl: 'https://relay.example',
          nsec: nostr.Keys.generate().nsec,
        ).copyWith(
          pushNotificationsEnabled: true,
          pushSubscriptionState: KuzzPushLeaseSubscriptionState.desired(
            desired: subscriptions,
          ),
        );
    await storage.save(community);
    await storage.saveActiveId(community.id);
    await tester.pumpWidget(
      ProviderScope(
        overrides: [
          savedPrefsProvider.overrideWithValue(prefs),
          if (buildSupported)
            kuzzPushBuildSupportedProvider.overrideWithValue(true),
          pushSubscriptionSyncProvider.overrideWithValue(
            KuzzPushSubscriptionSync.ok,
          ),
          authProvider.overrideWith(_Auth.new),
          communityStorageProvider.overrideWithValue(storage),
          communitySnapshotWriterProvider.overrideWithValue((_) async {}),
          currentRelayPushDescriptorProvider.overrideWith(
            (ref) => Future.value(_descriptor),
          ),
          appLifecycleProvider.overrideWith(_Lifecycle.new),
          kuzzPushAuthorizationStatusReaderProvider.overrideWithValue(
            () async => status,
          ),
        ],
        child: MaterialApp(
          theme: AppTheme.light(),
          home: SettingsPage(
            profileHeader: const SizedBox.shrink(),
            invitePageBuilder: (_) => const SizedBox.shrink(),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
  }

  testWidgets(
    'without Android push the card says it is not in this version',
    (tester) async {
      debugAndroidPushActiveOverride = false;
      await pumpSettings(
        tester,
        status: KuzzPushAuthorizationStatus.denied,
        buildSupported: false,
      );
      expect(find.text('Not available in this version'), findsOneWidget);
      expect(
        find.byKey(const ValueKey('push-notifications-open-settings')),
        findsNothing,
      );
      // Push that is on can still be turned off.
      final pushSwitch = tester.widget<Switch>(
        find.descendant(
          of: find.byKey(const ValueKey('push-notifications-enabled')),
          matching: find.byType(Switch),
        ),
      );
      expect(pushSwitch.onChanged, isNotNull);
    },
    variant: TargetPlatformVariant.only(TargetPlatform.android),
  );

  testWidgets(
    'denied shows Android copy and the settings row',
    (tester) async {
      debugAndroidPushActiveOverride = true;
      await pumpSettings(tester, status: KuzzPushAuthorizationStatus.denied);
      expect(
        find.text('Notifications are blocked for Kuzz in phone settings'),
        findsOneWidget,
      );
      expect(find.text('Open settings'), findsOneWidget);
      expect(find.textContaining('iOS'), findsNothing);
    },
    variant: TargetPlatformVariant.only(TargetPlatform.android),
  );

  testWidgets(
    'not asked yet says it waits for the permission',
    (tester) async {
      debugAndroidPushActiveOverride = true;
      await pumpSettings(
        tester,
        status: KuzzPushAuthorizationStatus.notDetermined,
      );
      expect(find.text('Waiting for notification permission'), findsOneWidget);
      expect(find.textContaining('iOS'), findsNothing);
    },
    variant: TargetPlatformVariant.only(TargetPlatform.android),
  );

  group('plugin calls', () {
    final calls = <MethodCall>[];
    setUp(() {
      calls.clear();
      TestWidgetsFlutterBinding.ensureInitialized();
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(androidPushChannel, (call) async {
            calls.add(call);
            return switch (call.method) {
              'permissionStatus' => 'notDetermined',
              'requestPermission' => 'authorized',
              _ => null,
            };
          });
    });
    tearDown(
      () => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(androidPushChannel, null),
    );

    test('status maps the plugin answer', () async {
      debugAndroidPushActiveOverride = true;
      expect(
        await readKuzzPushAuthorizationStatus(),
        KuzzPushAuthorizationStatus.notDetermined,
      );
    });

    test('off Android push the status is not asked', () async {
      debugAndroidPushActiveOverride = false;
      expect(
        await readKuzzPushAuthorizationStatus(),
        KuzzPushAuthorizationStatus.authorized,
      );
      await startKuzzPushRegistration();
      expect(calls, isEmpty);
    });
  });
}

const _descriptor = KuzzPushLeaseDescriptor(
  origin: 'wss://relay.example',
  executorKeyId: 'key',
  executorPubkey: 'pubkey',
  transport: 'fcm',
  maxLeaseTtlSeconds: 3600,
  maxContentLength: 4096,
  maxPlaintextLength: 4096,
  maxEndpointLength: 2048,
  maxStringLength: 512,
);

class _Lifecycle extends AppLifecycleNotifier {
  @override
  AppLifecycleState build() => AppLifecycleState.resumed;
}

class _Auth extends AuthNotifier {
  @override
  Future<AuthState> build() async =>
      const AuthState(status: AuthStatus.unauthenticated);
}
