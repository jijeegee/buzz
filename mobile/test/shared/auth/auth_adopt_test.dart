import 'dart:async';

import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:kuzz/shared/auth/auth.dart';
import 'package:kuzz/shared/auth/token/token.dart';
import 'package:shared_preferences/shared_preferences.dart';

import '../community/community_storage_test.dart' show FakeSecureStorage;
import 'token/token_auth_test_fakes.dart';

const _origin = 'https://relay.test';
final _principal = 'p' * 64;
final _key = SessionKey(_origin, _principal);

/// Saving the community and taking over the session commit together.
void main() {
  late FakeAuthServer server;
  late FakeRefreshTokenStore tokens;
  late CommunityStorage storage;
  late FakeClock clock;
  late ProviderContainer container;

  Future<TokenSessionController> signedInAttempt() async {
    final attempt = container.read(tokenSignInAttemptProvider)(_origin);
    addTearDown(attempt.dispose);
    expect(await attempt.signIn(), isTrue);
    return attempt;
  }

  List<String?> ended() => [
    for (final request in server.requests)
      if (request.url.path == '/auth/sessions/end')
        request.headers['Authorization'],
  ];

  Iterable<String> logouts() => server.requests
      .map((request) => request.url.path)
      .where((path) => path == '/auth/logout');

  setUp(() async {
    SharedPreferences.setMockInitialValues({});
    server = FakeAuthServer();
    tokens = FakeRefreshTokenStore();
    storage = CommunityStorage(secure: FakeSecureStorage());
    clock = FakeClock(DateTime.utc(2026, 10, 5, 12));
    container = ProviderContainer(
      overrides: [
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
        keyBackupRefreshTokenStoreProvider.overrideWithValue(
          FakeRefreshTokenStore(),
        ),
        installIdStoreProvider.overrideWithValue(
          InstallIdStore(storage: FakeSecureStorage()),
        ),
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
  });

  test('a new account gets its row and its session', () async {
    final attempt = await signedInAttempt();

    await container
        .read(authProvider.notifier)
        .authenticateWithTokenSession(
          relayUrl: _origin,
          principalId: _principal,
          attempt: attempt,
        );

    expect((await storage.loadAll()).single.pubkey, _principal);
    expect(tokens.data[_key]?.refreshToken, 'bzr_login');
    final controller = container.read(tokenSessionControllerProvider(_key));
    expect(controller.currentAccessToken, 'bzs_login');
    await attempt.discard();
    expect(ended(), isEmpty, reason: 'the attempt holds nothing anymore');
  });

  test('signing back in ends the replaced session, never the device', () async {
    final community = Community.create(
      name: 'Relay',
      relayUrl: _origin,
      pubkey: _principal,
      tokenAuth: true,
    );
    await storage.save(community);
    await storage.saveActiveId(community.id);
    tokens.data[_key] = StoredTokenSession(
      refreshToken: 'bzr_old',
      principalId: _principal,
    );
    server.refreshResponses.add(
      (_) => FakeAuthServer.rotated('bzs_old', 'bzr_old2'),
    );
    await container.read(tokenSessionControllerProvider(_key)).restore();
    final attempt = await signedInAttempt();

    await container
        .read(authProvider.notifier)
        .authenticateWithTokenSession(
          relayUrl: _origin,
          principalId: _principal,
          attempt: attempt,
        );
    await Future<void>.delayed(Duration.zero);

    expect(logouts(), isEmpty);
    expect(ended(), ['Bearer bzs_old']);
    expect(tokens.data[_key]?.refreshToken, 'bzr_login');
    expect((await storage.loadAll()), hasLength(1));
  });

  group('the community transition', () {
    late int transitions;
    late Community community;

    setUp(() async {
      transitions = 0;
      final unregister = container
          .read(communityTransitionProvider)
          .register(() async => transitions += 1);
      addTearDown(unregister);
      community = Community.create(
        name: 'Relay',
        relayUrl: _origin,
        pubkey: _principal,
        tokenAuth: true,
      );
      await storage.save(community);
      await storage.saveActiveId(community.id);
    });

    Future<void> signIn(String principal) async {
      final attempt = await signedInAttempt();
      await container
          .read(authProvider.notifier)
          .authenticateWithTokenSession(
            relayUrl: _origin,
            principalId: principal,
            attempt: attempt,
          );
    }

    test('stays out of the same account signing in again', () async {
      await signIn(_principal);
      expect(transitions, 0);
    });

    test('runs for another account', () async {
      await signIn('q' * 64);
      expect(transitions, 1);
    });
  });

  test('a session that cannot be stored leaves no row and no token', () async {
    final attempt = await signedInAttempt();
    tokens.writeError = StateError('keychain unavailable');

    await expectLater(
      container
          .read(authProvider.notifier)
          .authenticateWithTokenSession(
            relayUrl: _origin,
            principalId: _principal,
            attempt: attempt,
          ),
      throwsA(isA<TokenSessionException>()),
    );

    expect(await storage.loadAll(), isEmpty);
    expect(await storage.loadActiveId(), isNull);
    expect(tokens.data, isEmpty);
    expect(ended(), ['Bearer bzs_login']);
    expect(
      container.read(authProvider).value?.status,
      isNot(AuthStatus.authenticated),
    );
  });

  test('a failed takeover leaves the row with the session it had', () async {
    final row = Community.create(
      name: 'Saved',
      relayUrl: _origin,
      pubkey: _principal,
      tokenAuth: true,
    );
    await storage.save(row);
    await storage.saveActiveId(row.id);
    tokens.data[_key] = StoredTokenSession(
      refreshToken: 'bzr_saved',
      principalId: _principal,
    );
    server.refreshResponses.add(
      (_) => FakeAuthServer.rotated('bzs_saved', 'bzr_saved2'),
    );
    final controller = container.read(tokenSessionControllerProvider(_key));
    await controller.restore();
    final attempt = await signedInAttempt();
    tokens.writeError = StateError('keychain unavailable');

    await expectLater(
      container
          .read(authProvider.notifier)
          .authenticateWithTokenSession(
            relayUrl: _origin,
            principalId: _principal,
            attempt: attempt,
          ),
      throwsA(isA<TokenSessionException>()),
    );

    expect((await storage.loadAll()).single.signedOut, isFalse);
    expect(tokens.data[_key]?.refreshToken, 'bzr_saved2');
    expect(controller.currentAccessToken, 'bzs_saved');
  });

  test('a sign-in beats the sign-out queued behind it', () async {
    final row = Community.create(
      name: 'Saved',
      relayUrl: _origin,
      pubkey: _principal,
      tokenAuth: true,
    );
    await storage.save(row);
    await storage.saveActiveId(row.id);
    tokens.data[_key] = StoredTokenSession(
      refreshToken: 'bzr_saved',
      principalId: _principal,
    );
    server.refreshResponses.add(
      (_) => FakeAuthServer.authError(401, 'token_revoked'),
    );
    await container.read(authProvider.future);
    final controller = container.read(tokenSessionControllerProvider(_key));
    await controller.restore();
    final attempt = await signedInAttempt();
    final auth = container.read(authProvider.notifier);

    // The order the keeper produces: the sign-in is ahead in the queue when
    // the ended session's sign-out joins it.
    final hold = Completer<void>();
    unawaited(
      container
          .read(communityTransitionProvider)
          .runExclusive(() => hold.future),
    );
    final signIn = auth.authenticateWithTokenSession(
      relayUrl: _origin,
      principalId: _principal,
      attempt: attempt,
    );
    await pumpEventQueue();
    final signOut = auth.signOutCommunity(
      row.id,
      deviceOnly: true,
      isCurrent: controller.sessionFence(),
    );
    hold.complete();
    await Future.wait([signIn, signOut]);

    expect((await storage.loadAll()).single.signedOut, isFalse);
    expect(tokens.data[_key]?.refreshToken, 'bzr_login');
    expect(
      container.read(tokenSessionControllerProvider(_key)).currentAccessToken,
      'bzs_login',
    );
  });

  test('a failed takeover puts the previous row and active id back', () async {
    final existing = Community.create(
      name: 'Other',
      relayUrl: 'https://other.test',
      pubkey: 'o' * 64,
      tokenAuth: true,
    );
    final legacy = Community.create(
      name: 'Legacy',
      relayUrl: _origin,
      pubkey: _principal,
    );
    await storage.save(existing);
    await storage.save(legacy);
    await storage.saveActiveId(existing.id);
    final attempt = await signedInAttempt();
    tokens.writeError = StateError('keychain unavailable');

    await expectLater(
      container
          .read(authProvider.notifier)
          .authenticateWithTokenSession(
            relayUrl: _origin,
            principalId: _principal,
            attempt: attempt,
          ),
      throwsA(isA<TokenSessionException>()),
    );

    final rows = await storage.loadAll();
    expect(rows.singleWhere((row) => row.id == legacy.id).tokenAuth, isFalse);
    expect(await storage.loadActiveId(), existing.id);
  });
}
