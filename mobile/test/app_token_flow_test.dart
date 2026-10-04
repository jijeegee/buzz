import 'dart:async';

import 'package:buzz/app.dart';
import 'package:buzz/features/age_gate/age_signal_provider.dart';
import 'package:buzz/features/home/home_page.dart';
import 'package:buzz/features/sign_in/token_sign_in_page.dart';
import 'package:buzz/shared/auth/auth.dart';
import 'package:buzz/shared/auth/token/token.dart';
import 'package:buzz/shared/community/add_community_route.dart';
import 'package:buzz/shared/relay/relay.dart';
import 'package:buzz/shared/theme/theme_provider.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'shared/auth/token/token_auth_test_fakes.dart';
import 'shared/community/community_storage_test.dart' show FakeSecureStorage;

const _origin = 'https://relay.test';
final _principal = 'ab' * 32;

class _AllowedAgeSignalNotifier extends AgeSignalNotifier {
  @override
  AgeSignalState build() => AgeSignalState.allowed;

  @override
  Future<void> request() async {}
}

/// Keeps HomePage offline: no relay socket in widget tests.
class _OfflineRelaySessionNotifier extends RelaySessionNotifier {
  @override
  SessionState build() =>
      const SessionState(status: SessionStatus.disconnected);
}

/// The whole app over real auth, community and token-session providers with
/// fake HTTP, browser, clock and storage.
class _AppHarness {
  final server = FakeAuthServer();
  final tokens = FakeRefreshTokenStore();
  final storage = CommunityStorage(secure: FakeSecureStorage());
  final clock = FakeClock(DateTime.utc(2026, 10, 5, 12));
  late final ProviderContainer container;

  Future<void> storeSignedInCommunity() async {
    final community = Community.create(
      name: 'Relay',
      relayUrl: _origin,
      pubkey: _principal,
      tokenAuth: true,
    );
    await storage.save(community);
    await storage.saveActiveId(community.id);
    tokens.data[_origin] = StoredTokenSession(
      refreshToken: 'bzr_old',
      principalId: _principal,
    );
  }

  Future<void> pump(WidgetTester tester) async {
    SharedPreferences.setMockInitialValues({});
    final prefs = await SharedPreferences.getInstance();
    server.complete = (_) => jsonResponse({
      'principal_id': _principal,
      'device_id': 'device-1',
      'access': 'bzs_login',
      'refresh': 'bzr_login',
      'expires_in': 3600,
    });
    container = ProviderContainer(
      retry: (_, _) => null,
      overrides: [
        savedPrefsProvider.overrideWithValue(prefs),
        ageSignalProvider.overrideWith(_AllowedAgeSignalNotifier.new),
        relaySessionProvider.overrideWith(_OfflineRelaySessionNotifier.new),
        communityStorageProvider.overrideWithValue(storage),
        communitySnapshotWriterProvider.overrideWithValue((_) async {}),
        authHttpClientProvider.overrideWithValue(server.client),
        refreshTokenStoreProvider.overrideWithValue(tokens),
        webAuthLauncherProvider.overrideWithValue(
          FakeWebAuthLauncher(FakeWebAuthLauncher.success),
        ),
        sessionClockProvider.overrideWithValue(clock.call),
        sessionTimerFactoryProvider.overrideWithValue(clock.createTimer),
        authDeviceNameProvider.overrideWithValue('Test phone'),
      ],
    );
    addTearDown(container.dispose);
    await tester.pumpWidget(
      UncontrolledProviderScope(container: container, child: const App()),
    );
    await frames(tester);
  }

  /// Tears down HomePage and its periodic timers (presence heartbeat,
  /// offline history timeouts) before the test's timer check.
  Future<void> unmount(WidgetTester tester) async {
    await tester.pumpWidget(const SizedBox.shrink());
    container.dispose();
    await tester.pump(const Duration(seconds: 10));
  }
}

Future<void> frames(WidgetTester tester) async {
  for (var i = 0; i < 10; i++) {
    await tester.pump(const Duration(milliseconds: 16));
  }
}

void main() {
  testWidgets('Sign in with Google takes onboarding to the home page', (
    tester,
  ) async {
    final h = _AppHarness();
    await h.pump(tester);
    expect(find.byType(TokenSignInPage), findsOneWidget);
    expect(find.byType(HomePage), findsNothing);

    await tester.enterText(
      find.byKey(const Key('token-sign-in-relay-url')),
      'relay.test',
    );
    await tester.tap(find.byKey(const Key('token-sign-in-check')));
    await frames(tester);
    await tester.tap(find.byKey(const Key('token-sign-in-google')));
    await frames(tester);

    expect(find.byType(HomePage), findsOneWidget);
    expect(find.byType(TokenSignInPage), findsNothing);
    final stored = (await h.storage.loadAll()).single;
    expect(stored.tokenAuth, isTrue);
    expect(stored.pubkey, _principal);
    expect(h.tokens.data[_origin]?.refreshToken, 'bzr_login');
    await h.unmount(tester);
  });

  testWidgets('a stored session signs in automatically on launch', (
    tester,
  ) async {
    final h = _AppHarness();
    await h.storeSignedInCommunity();
    h.server.refreshResponses.add(
      (_) => FakeAuthServer.rotated('bzs_new', 'bzr_new'),
    );
    await h.pump(tester);

    expect(find.byType(HomePage), findsOneWidget);
    expect(find.byType(TokenSignInPage), findsNothing);
    expect(h.tokens.data[_origin]?.refreshToken, 'bzr_new');
    await h.unmount(tester);
  });

  testWidgets('a revoked stored session lands on sign-in, not home', (
    tester,
  ) async {
    final h = _AppHarness();
    await h.storeSignedInCommunity();
    h.server.refreshResponses.add(
      (_) => FakeAuthServer.authError(401, 'token_revoked'),
    );
    await h.pump(tester);

    expect(find.byType(HomePage), findsNothing);
    expect(find.byType(TokenSignInPage), findsOneWidget);
    expect(find.text('Signed out'), findsOneWidget);
    expect(find.byKey(const Key('token-sign-in-google')), findsOneWidget);
    expect(h.tokens.data, isEmpty);
    // The community itself is kept so the user can sign back in.
    expect(await h.storage.loadAll(), hasLength(1));
    await h.unmount(tester);
  });

  testWidgets('a refresh rejected with 401 mid-session returns to sign-in', (
    tester,
  ) async {
    final h = _AppHarness();
    await h.storeSignedInCommunity();
    h.server.refreshResponses
      ..add((_) => FakeAuthServer.rotated('bzs_new', 'bzr_new'))
      ..add((_) => FakeAuthServer.authError(401, 'token_revoked'));
    await h.pump(tester);
    expect(find.byType(HomePage), findsOneWidget);

    h.clock.advance(const Duration(minutes: 56));
    await frames(tester);

    expect(h.server.refreshCalls, 2);
    expect(find.byType(HomePage), findsNothing);
    expect(find.byKey(const Key('token-sign-in-google')), findsOneWidget);
    await h.unmount(tester);
  });

  testWidgets('the shared add-community route opens relay sign-in', (
    tester,
  ) async {
    final h = _AppHarness();
    await h.storeSignedInCommunity();
    h.server.refreshResponses.add(
      (_) => FakeAuthServer.rotated('bzs_new', 'bzr_new'),
    );
    await h.pump(tester);
    expect(find.byType(HomePage), findsOneWidget);

    unawaited(
      Navigator.of(
        tester.element(find.byType(HomePage)),
        rootNavigator: true,
      ).pushNamed(addCommunityRouteName),
    );
    await frames(tester);

    expect(find.byKey(const Key('token-sign-in-relay-url')), findsOneWidget);
    await h.unmount(tester);
  });
}
