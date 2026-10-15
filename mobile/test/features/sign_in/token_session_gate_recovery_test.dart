import 'package:buzz/features/sign_in/token_session_gate.dart';
import 'package:buzz/shared/auth/auth.dart';
import 'package:buzz/shared/auth/default_community.dart';
import 'package:buzz/shared/auth/token/token.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/auth/token/token_auth_test_fakes.dart';
import '../../shared/community/community_storage_test.dart';

const _origin = 'https://relay.test';
const _otherOrigin = 'https://other.test';
final _principal = 'p' * 64;

Future<void> frames(WidgetTester tester) async {
  for (var i = 0; i < 10; i++) {
    await tester.pump(const Duration(milliseconds: 16));
  }
}

/// A stalled or signed-out token community must never strand the user: the
/// recovery and locked views always offer removing the community and
/// switching to another one (AGENTS.md rule 6).
void main() {
  const home = Text('home', key: Key('gated-home'));

  late FakeAuthServer server;
  late FakeRefreshTokenStore tokens;
  late CommunityStorage storage;
  late ProviderContainer container;
  late Community stalled;
  late Community other;

  Future<void> pump(WidgetTester tester, {bool builtIn = false}) async {
    server = FakeAuthServer();
    tokens = FakeRefreshTokenStore();
    storage = CommunityStorage(secure: FakeSecureStorage());
    stalled = Community.create(
      name: 'Stalled relay',
      relayUrl: _origin,
      pubkey: _principal,
      tokenAuth: true,
    );
    other = Community.create(
      name: 'Other relay',
      relayUrl: _otherOrigin,
      pubkey: _principal,
      tokenAuth: true,
    );
    await storage.save(stalled);
    await storage.save(other);
    await storage.saveActiveId(stalled.id);
    final clock = FakeClock(DateTime.utc(2026, 10, 5, 12));
    container = ProviderContainer(
      overrides: [
        if (builtIn) defaultCommunityOriginProvider.overrideWithValue(_origin),
        communityStorageProvider.overrideWithValue(storage),
        communitySnapshotWriterProvider.overrideWithValue((_) async {}),
        communityPushLeaseRevocationEnqueuerProvider.overrideWithValue(
          (_) async => false,
        ),
        communityPushLeaseRevocationTriggerProvider.overrideWithValue(
          () async {},
        ),
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
    await container.read(authProvider.future);
    await tester.pumpWidget(
      UncontrolledProviderScope(
        container: container,
        child: const MaterialApp(
          home: TokenSessionGate(origin: _origin, child: home),
        ),
      ),
    );
    await frames(tester);
  }

  testWidgets('a stalled community without a saved sign-in can be removed', (
    tester,
  ) async {
    await pump(tester);
    tokens.readError = StateError('keychain locked');
    container.invalidate(tokenSessionControllerProvider(_origin));
    await frames(tester);

    expect(
      container.read(tokenSessionProvider(_origin)).status,
      TokenSessionStatus.stalled,
    );
    expect(find.byKey(const Key('gated-home')), findsNothing);
    expect(find.byKey(const Key('token-session-retry')), findsOneWidget);

    await tester.tap(find.byKey(const Key('community-recovery-remove')));
    await frames(tester);
    await tester.tap(
      find.byKey(const Key('community-recovery-remove-confirm')),
    );
    await frames(tester);

    final remaining = await storage.loadAll();
    expect(remaining.map((c) => c.id), [other.id]);
    expect(await storage.loadActiveId(), other.id);
  });

  testWidgets('a stalled community offers switching to another community', (
    tester,
  ) async {
    await pump(tester);
    tokens.readError = StateError('keychain locked');
    container.invalidate(tokenSessionControllerProvider(_origin));
    await frames(tester);

    await tester.tap(find.byKey(Key('community-recovery-switch-${other.id}')));
    await frames(tester);

    expect(await storage.loadActiveId(), other.id);
    expect((await storage.loadAll()).length, 2);
  });

  testWidgets('a signed-out community can be removed from the locked page', (
    tester,
  ) async {
    await pump(tester);

    // No saved sign-in: the gate shows the locked sign-in page.
    expect(find.byKey(const Key('token-sign-in-google')), findsOneWidget);
    expect(
      find.byKey(Key('community-recovery-switch-${other.id}')),
      findsOneWidget,
    );

    await tester.tap(find.byKey(const Key('community-recovery-remove')));
    await frames(tester);
    await tester.tap(
      find.byKey(const Key('community-recovery-remove-confirm')),
    );
    await frames(tester);

    expect((await storage.loadAll()).map((c) => c.id), [other.id]);
  });

  group('with the built-in server', () {
    testWidgets('a stalled session offers sign in again and sign out only', (
      tester,
    ) async {
      await pump(tester, builtIn: true);
      tokens.readError = StateError('keychain locked');
      container.invalidate(tokenSessionControllerProvider(_origin));
      await frames(tester);

      expect(find.byKey(const Key('token-session-retry')), findsOneWidget);
      expect(
        find.byKey(const Key('token-session-sign-in-again')),
        findsOneWidget,
      );
      expect(find.byKey(const Key('community-recovery-remove')), findsNothing);
      expect(
        find.byKey(Key('community-recovery-switch-${other.id}')),
        findsNothing,
      );
      expect(find.text('Other relay'), findsNothing);

      await tester.tap(find.byKey(const Key('community-recovery-sign-out')));
      await frames(tester);

      // Ordinary sign-out: every community stays stored, signed out.
      final remaining = await storage.loadAll();
      expect(remaining.length, 2);
      expect(remaining.every((community) => community.signedOut), isTrue);
    });

    testWidgets('the locked page has Google sign-in and sign out only', (
      tester,
    ) async {
      await pump(tester, builtIn: true);

      expect(find.byKey(const Key('token-sign-in-google')), findsOneWidget);
      expect(find.byKey(const Key('token-sign-in-relay-url')), findsNothing);
      expect(
        find.byKey(const Key('community-recovery-sign-out')),
        findsOneWidget,
      );
      expect(find.byKey(const Key('community-recovery-remove')), findsNothing);
      expect(
        find.byKey(Key('community-recovery-switch-${other.id}')),
        findsNothing,
      );
      expect(find.textContaining('relay.test'), findsNothing);
    });
  });
}
