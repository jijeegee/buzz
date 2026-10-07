import 'package:buzz/features/sign_in/token_session_gate.dart';
import 'package:buzz/features/sign_in/token_sign_in_page.dart';
import 'package:buzz/shared/auth/auth.dart';
import 'package:buzz/shared/auth/token/token.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/auth/token/token_auth_test_fakes.dart';
import '../../shared/community/community_storage_test.dart';

const _origin = 'https://relay.test';
final _principal = 'p' * 64;

class _RecordingAuthNotifier extends AuthNotifier {
  final List<({String relayUrl, String principalId})> tokenSignIns = [];

  @override
  Future<AuthState> build() async =>
      const AuthState(status: AuthStatus.unauthenticated);

  @override
  Future<void> authenticateWithTokenSession({
    required String relayUrl,
    required String principalId,
    bool Function()? isCurrent,
    bool resumeSignedOut = false,
  }) async {
    tokenSignIns.add((relayUrl: relayUrl, principalId: principalId));
  }
}

class _Harness {
  final server = FakeAuthServer();
  final store = FakeRefreshTokenStore();
  final launcher = FakeWebAuthLauncher(FakeWebAuthLauncher.success);
  final clock = FakeClock(DateTime.utc(2026, 10, 5, 12));
  final auth = _RecordingAuthNotifier();

  late final container = ProviderContainer(
    overrides: [
      authHttpClientProvider.overrideWithValue(server.client),
      refreshTokenStoreProvider.overrideWithValue(store),
      webAuthLauncherProvider.overrideWithValue(launcher),
      sessionClockProvider.overrideWithValue(clock.call),
      sessionTimerFactoryProvider.overrideWithValue(clock.createTimer),
      authDeviceNameProvider.overrideWithValue('Test phone'),
      authProvider.overrideWith(() => auth),
      communityStorageProvider.overrideWithValue(
        CommunityStorage(secure: FakeSecureStorage()),
      ),
    ],
  );

  Future<void> pump(WidgetTester tester, Widget home) async {
    addTearDown(container.dispose);
    await tester.pumpWidget(
      UncontrolledProviderScope(
        container: container,
        child: MaterialApp(home: home),
      ),
    );
    await frames(tester);
  }
}

/// Lets fake HTTP, storage and microtasks settle without waiting for the
/// (endless) loading indicator animation.
Future<void> frames(WidgetTester tester) async {
  for (var i = 0; i < 10; i++) {
    await tester.pump(const Duration(milliseconds: 16));
  }
}

void main() {
  testWidgets(
    'relay check, Sign in with Google, fake browser: signed in and recorded',
    (tester) async {
      final h = _Harness();
      await h.pump(tester, const TokenSignInPage());
      expect(find.byKey(const Key('token-sign-in-google')), findsNothing);

      await tester.enterText(
        find.byKey(const Key('token-sign-in-relay-url')),
        'wss://Relay.Test/',
      );
      await tester.tap(find.byKey(const Key('token-sign-in-check')));
      await frames(tester);
      expect(h.server.requests.first.url.toString(), '$_origin/');
      expect(
        h.container.read(tokenSessionProvider(_origin)).status,
        TokenSessionStatus.signedOut,
      );

      await tester.tap(find.byKey(const Key('token-sign-in-google')));
      await frames(tester);

      final opened = h.launcher.opened.single;
      expect(opened.path, '/auth/oidc/google/start');
      expect(opened.queryParameters['client'], 'mobile');
      expect(
        opened.queryParameters['redirect_uri'],
        'xyz.block.buzz://auth/cb',
      );
      expect(h.launcher.schemes.single, 'xyz.block.buzz');
      expect(
        h.store.data[_origin],
        StoredTokenSession(refreshToken: 'bzr_login', principalId: _principal),
      );
      final session = h.container.read(tokenSessionProvider(_origin));
      expect(session.status, TokenSessionStatus.signedIn);
      expect(session.principalId, _principal);
      expect(
        h.container
            .read(tokenSessionControllerProvider(_origin))
            .currentAccessToken,
        'bzs_login',
      );
      expect(h.auth.tokenSignIns, [
        (relayUrl: _origin, principalId: _principal),
      ]);
    },
  );

  testWidgets('a legacy relay offers no Google sign-in', (tester) async {
    final h = _Harness();
    h.server.nip11 = (_) => jsonResponse({'name': 'Legacy'});
    await h.pump(tester, const TokenSignInPage());

    await tester.enterText(
      find.byKey(const Key('token-sign-in-relay-url')),
      'relay.test',
    );
    await tester.tap(find.byKey(const Key('token-sign-in-check')));
    await frames(tester);

    expect(find.byKey(const Key('token-sign-in-google')), findsNothing);
    expect(find.textContaining('does not support Google'), findsOneWidget);
  });

  testWidgets('an unreachable relay is reported, not treated as legacy', (
    tester,
  ) async {
    final h = _Harness();
    h.server.nip11 = (_) => jsonResponse({'error': 'down'}, status: 503);
    await h.pump(tester, const TokenSignInPage());

    await tester.enterText(
      find.byKey(const Key('token-sign-in-relay-url')),
      'relay.test',
    );
    await tester.tap(find.byKey(const Key('token-sign-in-check')));
    await frames(tester);

    expect(find.textContaining('HTTP 503'), findsOneWidget);
    expect(find.textContaining('does not support Google'), findsNothing);
  });

  testWidgets('a cancelled browser records nothing and can be retried', (
    tester,
  ) async {
    final h = _Harness();
    h.launcher.error = const WebAuthCancelledException();
    await h.pump(tester, const TokenSignInPage(lockedOrigin: _origin));

    await tester.tap(find.byKey(const Key('token-sign-in-google')));
    await frames(tester);

    expect(
      h.container.read(tokenSessionProvider(_origin)).status,
      TokenSessionStatus.signedOut,
    );
    expect(h.store.data, isEmpty);
    expect(h.auth.tokenSignIns, isEmpty);
    final button = tester.widget<FilledButton>(
      find.byKey(const Key('token-sign-in-google')),
    );
    expect(button.onPressed, isNotNull);
  });

  group('TokenSessionGate', () {
    const home = Text('home', key: Key('gated-home'));

    testWidgets('a stored session restores straight to the gated child', (
      tester,
    ) async {
      final h = _Harness();
      h.store.data[_origin] = StoredTokenSession(
        refreshToken: 'bzr_old',
        principalId: _principal,
      );
      h.server.refreshResponses.add(
        (_) => FakeAuthServer.rotated('bzs_new', 'bzr_new'),
      );
      await h.pump(
        tester,
        const TokenSessionGate(origin: _origin, child: home),
      );

      expect(find.byKey(const Key('gated-home')), findsOneWidget);
      expect(h.store.data[_origin]?.refreshToken, 'bzr_new');
    });

    testWidgets('a revoked session lands on the sign-in page', (tester) async {
      final h = _Harness();
      h.store.data[_origin] = StoredTokenSession(
        refreshToken: 'bzr_old',
        principalId: _principal,
      );
      h.server.refreshResponses.add(
        (_) => FakeAuthServer.authError(401, 'token_revoked'),
      );
      await h.pump(
        tester,
        const TokenSessionGate(origin: _origin, child: home),
      );

      expect(find.byKey(const Key('gated-home')), findsNothing);
      expect(find.byKey(const Key('token-sign-in-google')), findsOneWidget);
      expect(h.store.data, isEmpty);
    });

    testWidgets('a refresh rejected mid-session returns to the sign-in page', (
      tester,
    ) async {
      final h = _Harness();
      h.store.data[_origin] = StoredTokenSession(
        refreshToken: 'bzr_old',
        principalId: _principal,
      );
      h.server.refreshResponses
        ..add((_) => FakeAuthServer.rotated('bzs_new', 'bzr_new'))
        ..add((_) => FakeAuthServer.authError(401, 'token_revoked'));
      await h.pump(
        tester,
        const TokenSessionGate(origin: _origin, child: home),
      );
      expect(find.byKey(const Key('gated-home')), findsOneWidget);

      // The proactive refresh fires 5 minutes before the hour-long token
      // expires and the relay has revoked the session meanwhile.
      h.clock.advance(const Duration(minutes: 56));
      await frames(tester);

      expect(h.server.refreshCalls, 2);
      expect(find.byKey(const Key('gated-home')), findsNothing);
      expect(find.byKey(const Key('token-sign-in-google')), findsOneWidget);
      expect(h.store.data, isEmpty);
    });

    testWidgets('an unreachable relay keeps home and a recovery banner', (
      tester,
    ) async {
      final h = _Harness();
      h.store.data[_origin] = StoredTokenSession(
        refreshToken: 'bzr_old',
        principalId: _principal,
      );
      h.server.refreshResponses.add(
        (_) => jsonResponse({'error': 'unavailable'}, status: 503),
      );
      await h.pump(
        tester,
        const TokenSessionGate(origin: _origin, child: home),
      );

      // The saved principal keeps the app usable (cached content, settings,
      // switching or removing the community) under a non-blocking banner.
      expect(find.byKey(const Key('gated-home')), findsOneWidget);
      expect(find.byKey(const Key('token-session-banner')), findsOneWidget);
      expect(find.byKey(const Key('token-session-retry')), findsOneWidget);
      expect(
        find.byKey(const Key('token-session-sign-in-again')),
        findsOneWidget,
      );
      expect(h.store.data[_origin]?.refreshToken, 'bzr_old');

      h.server.refreshResponses.add(
        (_) => FakeAuthServer.rotated('bzs_new', 'bzr_new'),
      );
      await tester.tap(find.byKey(const Key('token-session-retry')));
      await frames(tester);
      expect(find.byKey(const Key('gated-home')), findsOneWidget);
      expect(find.byKey(const Key('token-session-banner')), findsNothing);
    });
  });
}
