import 'package:buzz/shared/auth/auth.dart';
import 'package:buzz/shared/auth/token/token.dart';
import 'package:buzz/shared/community/community_membership_provider.dart';
import 'package:buzz/shared/push/push_relay_capability_provider.dart';
import 'package:buzz/shared/theme/theme.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:hooks_riverpod/misc.dart';
import 'package:shared_preferences/shared_preferences.dart';

import '../../shared/auth/account/fake_account_server.dart';
import '../../shared/auth/token/token_auth_test_fakes.dart';
import '../../shared/community/community_storage_test.dart';

const tokenOrigin = 'https://relay.test';
final tokenPrincipal = 'ab' * 32;

/// A signed-in token community: real auth, community, token-session and
/// account providers over fake HTTP and storage.
class TokenSettingsHarness {
  TokenSettingsHarness({this.nsec});

  /// A legacy key that should stay hidden for a token community.
  final String? nsec;

  final account = FakeAccountServer();
  FakeAuthServer get server => account.auth;
  final tokens = FakeRefreshTokenStore();
  final storage = CommunityStorage(secure: FakeSecureStorage());
  final clock = FakeClock(DateTime.utc(2026, 10, 5, 12));
  late final Community community = Community.create(
    name: 'Relay',
    relayUrl: tokenOrigin,
    pubkey: tokenPrincipal,
    nsec: nsec,
    tokenAuth: true,
  );

  late final ProviderContainer container;

  Future<void> pump(
    WidgetTester tester,
    Widget home, {
    List<Override> overrides = const [],
  }) async {
    SharedPreferences.setMockInitialValues({});
    final prefs = await SharedPreferences.getInstance();
    await storage.save(community);
    await storage.saveActiveId(community.id);
    tokens.data[tokenOrigin] = StoredTokenSession(
      refreshToken: 'bzr_old',
      principalId: tokenPrincipal,
    );
    container = ProviderContainer(
      overrides: [
        savedPrefsProvider.overrideWithValue(prefs),
        communityStorageProvider.overrideWithValue(storage),
        communitySnapshotWriterProvider.overrideWithValue((_) async {}),
        communityPushLeaseRevocationEnqueuerProvider.overrideWithValue(
          (_) async => false,
        ),
        communityPushLeaseRevocationTriggerProvider.overrideWithValue(
          () async {},
        ),
        authHttpClientProvider.overrideWithValue(account.client),
        refreshTokenStoreProvider.overrideWithValue(tokens),
        webAuthLauncherProvider.overrideWithValue(
          FakeWebAuthLauncher(FakeWebAuthLauncher.success),
        ),
        sessionClockProvider.overrideWithValue(clock.call),
        sessionTimerFactoryProvider.overrideWithValue(clock.createTimer),
        authDeviceNameProvider.overrideWithValue('Test phone'),
        currentCommunityRoleProvider.overrideWithValue(
          const AsyncData<CommunityMemberRole?>(CommunityMemberRole.member),
        ),
        currentRelayPushDescriptorProvider.overrideWith((ref) async => null),
        ...overrides,
      ],
    );
    addTearDown(container.dispose);
    await container.read(authProvider.future);
    await tester.pumpWidget(
      UncontrolledProviderScope(
        container: container,
        child: MaterialApp(theme: AppTheme.light(), home: home),
      ),
    );
    await frames(tester);
  }
}

/// Lets fake HTTP, storage and microtasks settle without waiting for
/// endless loading animations.
Future<void> frames(WidgetTester tester) async {
  for (var i = 0; i < 10; i++) {
    await tester.pump(const Duration(milliseconds: 16));
  }
}
