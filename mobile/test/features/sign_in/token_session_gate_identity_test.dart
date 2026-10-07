import 'package:buzz/features/sign_in/token_session_gate.dart';
import 'package:buzz/shared/auth/auth.dart';
import 'package:buzz/shared/auth/token/token.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/auth/token/token_auth_test_fakes.dart';
import '../../shared/community/community_storage_test.dart';

const _origin = 'https://relay.test';
final _oldPrincipal = 'q' * 64;
final _newPrincipal = 'p' * 64;

/// The real notifier, with injectable failures for the identity write.
class _FlakyAuthNotifier extends AuthNotifier {
  int failuresLeft = 0;
  int attempts = 0;

  @override
  Future<void> authenticateWithTokenSession({
    required String relayUrl,
    required String principalId,
    bool Function()? isCurrent,
    bool resumeSignedOut = false,
  }) async {
    attempts += 1;
    if (failuresLeft > 0) {
      failuresLeft -= 1;
      throw StateError('keychain unavailable');
    }
    await super.authenticateWithTokenSession(
      relayUrl: relayUrl,
      principalId: principalId,
    );
  }
}

Future<void> frames(WidgetTester tester) async {
  for (var i = 0; i < 10; i++) {
    await tester.pump(const Duration(milliseconds: 16));
  }
}

void main() {
  const home = Text('home', key: Key('gated-home'));

  late FakeAuthServer server;
  late FakeRefreshTokenStore tokens;
  late CommunityStorage storage;
  late _FlakyAuthNotifier auth;
  late ProviderContainer container;

  Future<void> pump(WidgetTester tester) async {
    server = FakeAuthServer();
    tokens = FakeRefreshTokenStore();
    storage = CommunityStorage(secure: FakeSecureStorage());
    auth = _FlakyAuthNotifier();
    // Stored with the account it was last signed in to; the refresh now
    // belongs to a different principal (re-sign-in with another account).
    final community = Community.create(
      name: 'Relay',
      relayUrl: _origin,
      pubkey: _oldPrincipal,
      tokenAuth: true,
    );
    await storage.save(community);
    await storage.saveActiveId(community.id);
    tokens.data[_origin] = StoredTokenSession(
      refreshToken: 'bzr_old',
      principalId: _newPrincipal,
    );
    server.refreshResponses.add(
      (_) => FakeAuthServer.rotated('bzs_new', 'bzr_new'),
    );
    final clock = FakeClock(DateTime.utc(2026, 10, 5, 12));
    container = ProviderContainer(
      overrides: [
        authProvider.overrideWith(() => auth),
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
    await container.read(authProvider.future);
  }

  Future<void> show(WidgetTester tester) async {
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

  testWidgets('a new principal is recorded before the gated child shows', (
    tester,
  ) async {
    await pump(tester);
    await show(tester);

    expect(find.byKey(const Key('gated-home')), findsOneWidget);
    expect((await storage.loadAll()).single.pubkey, _newPrincipal);
  });

  testWidgets(
    'a failed identity write is surfaced with retry, not only logged',
    (tester) async {
      await pump(tester);
      auth.failuresLeft = 1;
      await show(tester);

      expect(find.byKey(const Key('gated-home')), findsNothing);
      expect(find.textContaining('keychain unavailable'), findsOneWidget);
      expect((await storage.loadAll()).single.pubkey, _oldPrincipal);

      await tester.tap(find.byKey(const Key('token-identity-retry')));
      await frames(tester);

      expect(auth.attempts, 2);
      expect(find.byKey(const Key('gated-home')), findsOneWidget);
      expect((await storage.loadAll()).single.pubkey, _newPrincipal);
    },
  );
}
