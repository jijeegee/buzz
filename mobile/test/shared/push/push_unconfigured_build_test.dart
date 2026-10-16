import 'package:kuzz/features/settings/settings_page.dart';
import 'package:kuzz/shared/community/community.dart';
import 'package:kuzz/shared/community/community_provider.dart';
import 'package:kuzz/shared/push/dev_push_lease.dart';
import 'package:kuzz/shared/push/push_bootstrap.dart';
import 'package:kuzz/shared/push/push_bridge.dart';
import 'package:kuzz/shared/push/push_relay_capability_provider.dart';
import 'package:kuzz/shared/relay/relay_provider.dart';
import 'package:kuzz/shared/relay/relay_session.dart';
import 'package:kuzz/shared/theme/theme.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:shared_preferences/shared_preferences.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  group('artifact without a gateway define', () {
    test(
      'does not discover push capability or enter the opted-in lifecycle',
      () async {
        final container = ProviderContainer(
          overrides: [
            relaySessionProvider.overrideWith(
              () => throw StateError('must not start relay discovery'),
            ),
          ],
        );
        addTearDown(container.dispose);
        expect(
          await container.read(currentRelayPushDescriptorProvider.future),
          isNull,
        );
        expect(
          kuzzPushLifecycleEnabled(
            community: Community.create(
              name: 'Team',
              relayUrl: 'wss://relay.example',
            ).copyWith(pushNotificationsEnabled: true),
            descriptor: const KuzzPushLeaseDescriptor(
              origin: 'wss://relay.example',
              executorKeyId: 'key',
              executorPubkey: 'pubkey',
              transport: 'apns',
              maxLeaseTtlSeconds: 3600,
              maxContentLength: 4096,
              maxPlaintextLength: 4096,
              maxEndpointLength: 2048,
              maxStringLength: 512,
            ),
          ),
          isFalse,
        );
      },
    );

    test('never requests native registration or gateway grants', () async {
      debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      const channel = MethodChannel('kuzz/push');
      final calls = <String>[];
      final messenger =
          TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
      messenger.setMockMethodCallHandler(channel, (call) async {
        calls.add(call.method);
        return null;
      });
      addTearDown(() => messenger.setMockMethodCallHandler(channel, null));
      await startKuzzPushRegistration();
      expect(await readKuzzPushEndpointGrants(), isEmpty);
      expect(calls, isEmpty);
    });

    testWidgets('Settings hides notifications without push capability', (
      tester,
    ) async {
      debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      SharedPreferences.setMockInitialValues({});
      final prefs = await SharedPreferences.getInstance();
      await tester.pumpWidget(
        ProviderScope(
          overrides: [
            savedPrefsProvider.overrideWithValue(prefs),
            activeCommunityProvider.overrideWith(
              (ref) async => Community.create(
                name: 'Team',
                relayUrl: 'wss://relay.example',
              ),
            ),
            kuzzPushAuthorizationStatusReaderProvider.overrideWithValue(
              () async =>
                  throw StateError('must not query notification permission'),
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
      expect(find.text('Notifications'), findsNothing);
      expect(find.text('Push notifications'), findsNothing);
      expect(find.byType(Switch), findsNothing);
      expect(tester.takeException(), isNull);
      debugDefaultTargetPlatformOverride = null;
    });
  }, skip: Env.pushGatewayConfigured);
}
