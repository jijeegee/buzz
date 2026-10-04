import 'dart:async';

import 'package:buzz/shared/auth/token/token.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;

import 'token_auth_test_fakes.dart';

const origin = 'https://relay.example';
const principal = 'aa11';
const stored1 = StoredTokenSession(
  refreshToken: 'bzr_1',
  principalId: principal,
);

class Harness {
  Harness({StoredTokenSession? stored}) {
    if (stored != null) store.data[origin] = stored;
  }

  final server = FakeAuthServer();
  final store = FakeRefreshTokenStore();
  final clock = FakeClock(DateTime.utc(2026, 1, 1));
  final launcher = FakeWebAuthLauncher(FakeWebAuthLauncher.success);
  final List<TokenSessionState> states = [];

  late final TokenSessionController controller = () {
    final controller = TokenSessionController(
      origin: origin,
      api: AuthApi(origin: origin, client: server.client),
      store: store,
      launcher: launcher,
      clock: clock.call,
      timerFactory: clock.createTimer,
      deviceName: 'Test phone',
    );
    controller.addListener(states.add);
    return controller;
  }();

  TokenSessionStatus get status => controller.state.status;

  List<http.Request> get logouts =>
      server.requests.where((r) => r.url.path == '/auth/logout').toList();

  /// Restore into a signed-in session holding `bzs_2` / `bzr_2`.
  Future<void> signedIn({int ttl = 3600}) async {
    store.data[origin] = stored1;
    server.refreshResponses.add(
      (_) => FakeAuthServer.rotated('bzs_2', 'bzr_2', ttl: ttl),
    );
    await controller.restore();
    expect(status, TokenSessionStatus.signedIn);
  }
}

void main() {
  group('restore', () {
    test(
      'without a stored session is signed out and never calls the relay',
      () async {
        final h = Harness();
        await h.controller.restore();
        expect(h.status, TokenSessionStatus.signedOut);
        expect(h.server.requests, isEmpty);
        expect(h.controller.currentAccessToken, isNull);
      },
    );

    test(
      'persists the rotated refresh before publishing the new access token',
      () async {
        final h = Harness(stored: stored1);
        h.server.refreshResponses.add(
          (_) => FakeAuthServer.rotated('bzs_2', 'bzr_2'),
        );
        final gate = h.store.writeGate = Completer<void>();
        final restoring = h.controller.restore();
        await settle();
        expect(h.server.refreshCalls, 1);
        expect(
          h.controller.currentAccessToken,
          isNull,
          reason: 'not published before persist',
        );
        expect(h.status, TokenSessionStatus.restoring);
        gate.complete();
        await restoring;
        expect(
          h.store.data[origin],
          const StoredTokenSession(
            refreshToken: 'bzr_2',
            principalId: principal,
          ),
        );
        expect(h.controller.currentAccessToken, 'bzs_2');
        expect(h.controller.state.principalId, principal);
        expect(h.status, TokenSessionStatus.signedIn);
      },
    );

    test(
      'a failed persist is terminal: nothing is published and the orphan is revoked',
      () async {
        final h = Harness(stored: stored1);
        h.server.refreshResponses.add(
          (_) => FakeAuthServer.rotated('bzs_2', 'bzr_2'),
        );
        h.store.writeError = StateError('keychain unavailable');
        await h.controller.restore();
        await settle();
        expect(h.status, TokenSessionStatus.signedOut);
        expect(h.controller.state.errorMessage, isNotNull);
        expect(h.controller.currentAccessToken, isNull);
        expect(
          h.states.any((s) => s.status == TokenSessionStatus.signedIn),
          isFalse,
        );
        expect(h.logouts.single.headers['Authorization'], 'Bearer bzs_2');
        expect(h.clock.active, isEmpty);
      },
    );

    test('a terminal 401 signs out and deletes the stored session', () async {
      final h = Harness(stored: stored1);
      h.server.refreshResponses.add(
        (_) => FakeAuthServer.authError(401, 'refresh_reused'),
      );
      await h.controller.restore();
      expect(h.status, TokenSessionStatus.signedOut);
      expect(h.store.data, isEmpty);
      expect(h.clock.active, isEmpty);
    });

    test('a 400 is terminal too', () async {
      final h = Harness(stored: stored1);
      h.server.refreshResponses.add(
        (_) => FakeAuthServer.authError(400, 'invalid_request'),
      );
      await h.controller.restore();
      expect(h.status, TokenSessionStatus.signedOut);
      expect(h.store.data, isEmpty);
    });

    for (final status in [403, 404]) {
      test('a $status from /auth/refresh is not a revocation (token auth off '
          'answers like any unrouted path): the session is kept', () async {
        final h = Harness(stored: stored1);
        h.server.refreshResponses.add((_) => http.Response('', status));
        await h.controller.restore();
        expect(h.status, TokenSessionStatus.retrying);
        expect(h.controller.state.principalId, principal);
        expect(h.store.data[origin], stored1);
      });
    }

    test(
      'transient failures back off 5s..300s, then stall with the session kept',
      () async {
        final h = Harness(stored: stored1);
        for (var i = 0; i < 6; i++) {
          h.server.refreshResponses.add(
            (_) => FakeAuthServer.authError(503, 'unavailable'),
          );
        }
        await h.controller.restore();
        expect(h.status, TokenSessionStatus.retrying);
        expect(h.controller.state.principalId, principal);
        for (final delay in TokenSessionController.retryDelays) {
          expect(h.clock.active.single.delay, delay);
          expect(h.controller.state.nextRetryAt, h.clock.now.add(delay));
          h.clock.advance(delay);
          await settle();
        }
        expect(h.server.refreshCalls, 6);
        expect(h.status, TokenSessionStatus.stalled);
        expect(h.clock.active, isEmpty, reason: 'no unbounded retry loop');
        expect(
          h.store.data[origin],
          stored1,
          reason: 'transient failures keep the session',
        );
        expect(await h.controller.ensureFreshAccessToken(), isNull);
        expect(
          h.server.refreshCalls,
          6,
          reason: 'stalled sessions do not refresh on demand',
        );

        h.server.refreshResponses.add(
          (_) => FakeAuthServer.rotated('bzs_2', 'bzr_2'),
        );
        await h.controller.retryNow();
        expect(h.status, TokenSessionStatus.signedIn);
        expect(h.controller.currentAccessToken, 'bzs_2');
      },
    );

    test('an unreadable keychain stalls instead of signing out', () async {
      final h = Harness(stored: stored1);
      h.store.readError = StateError('locked');
      await h.controller.restore();
      expect(h.status, TokenSessionStatus.stalled);
      h.store.readError = null;
      h.server.refreshResponses.add(
        (_) => FakeAuthServer.rotated('bzs_2', 'bzr_2'),
      );
      await h.controller.retryNow();
      expect(h.status, TokenSessionStatus.signedIn);
    });
  });

  group('refresh', () {
    test('concurrent callers share one rotation', () async {
      final h = Harness(stored: stored1);
      h.server.holdRefreshes = true;
      final restoring = h.controller.restore();
      final a = h.controller.ensureFreshAccessToken();
      final b = h.controller.ensureFreshAccessToken();
      await settle();
      expect(h.server.heldRefreshes.length, 1);
      h.server.heldRefreshes.removeFirst().complete(
        FakeAuthServer.rotated('bzs_2', 'bzr_2'),
      );
      await restoring;
      expect(await a, 'bzs_2');
      expect(await b, 'bzs_2');
      expect(h.server.refreshCalls, 1);
    });

    test('a proactive timer refreshes five minutes before expiry', () async {
      final h = Harness();
      await h.signedIn();
      expect(h.clock.active.single.delay, const Duration(minutes: 55));
      h.server.refreshResponses.add((request) {
        expect(request.body, contains('bzr_2'));
        return FakeAuthServer.rotated('bzs_3', 'bzr_3');
      });
      h.clock.advance(const Duration(minutes: 55));
      await settle();
      expect(h.controller.currentAccessToken, 'bzs_3');
      expect(h.store.data[origin]!.refreshToken, 'bzr_3');
      expect(h.clock.active.single.delay, const Duration(minutes: 55));
    });

    test(
      'access-token listeners hear sign-in and every rotation, not sign-out',
      () async {
        final h = Harness();
        final tokens = <String>[];
        final remove = h.controller.addAccessTokenListener(tokens.add);
        await h.signedIn();
        h.server.refreshResponses.add(
          (_) => FakeAuthServer.rotated('bzs_3', 'bzr_3'),
        );
        h.clock.advance(const Duration(minutes: 55));
        await settle();
        expect(
          h.states.where((s) => s.status == TokenSessionStatus.signedIn).length,
          1,
          reason: 'state listeners do not see signedIn -> signedIn rotations',
        );
        expect(tokens, ['bzs_2', 'bzs_3']);
        remove();
        h.server.refreshResponses.add(
          (_) => FakeAuthServer.rotated('bzs_4', 'bzr_4'),
        );
        h.clock.advance(const Duration(minutes: 55));
        await settle();
        expect(h.controller.currentAccessToken, 'bzs_4');
        expect(tokens, ['bzs_2', 'bzs_3'], reason: 'unsubscribed');
      },
    );

    test('short-lived tokens cannot drive a tight refresh loop', () async {
      final h = Harness();
      await h.signedIn(ttl: 60);
      expect(h.clock.active.single.delay, TokenSessionController.minTimerDelay);
    });

    test(
      'a transient proactive failure keeps the still-valid token and retries',
      () async {
        final h = Harness();
        await h.signedIn();
        h.server.refreshResponses.add(
          (_) => FakeAuthServer.authError(503, 'unavailable'),
        );
        h.clock.advance(const Duration(minutes: 55));
        await settle();
        expect(h.status, TokenSessionStatus.signedIn);
        expect(h.controller.currentAccessToken, 'bzs_2');
        expect(h.clock.active.single.delay, const Duration(seconds: 5));
      },
    );

    test(
      'a token near expiry is not handed out; ensureFresh rotates it',
      () async {
        final h = Harness();
        await h.signedIn();
        h.clock.now = h.clock.now.add(const Duration(minutes: 59, seconds: 30));
        h.server.refreshResponses.add(
          (_) => FakeAuthServer.rotated('bzs_3', 'bzr_3'),
        );
        expect(await h.controller.ensureFreshAccessToken(), 'bzs_3');
      },
    );

    test(
      'token_expired forces one refresh unless the token already rotated',
      () async {
        final h = Harness();
        await h.signedIn();
        expect(await h.controller.refreshAfterTokenExpired('bzs_old'), 'bzs_2');
        expect(
          h.server.refreshCalls,
          1,
          reason: 'stale rejection does not rotate again',
        );
        h.server.refreshResponses.add(
          (_) => FakeAuthServer.rotated('bzs_3', 'bzr_3'),
        );
        expect(await h.controller.refreshAfterTokenExpired('bzs_2'), 'bzs_3');
        expect(h.server.refreshCalls, 2);
      },
    );
  });

  group('sign in', () {
    test(
      'logs in, stores the refresh, and publishes the access token',
      () async {
        final h = Harness();
        await h.controller.restore();
        expect(await h.controller.signIn(), isTrue);
        expect(h.status, TokenSessionStatus.signedIn);
        expect(h.controller.state.principalId, 'p' * 64);
        expect(h.controller.currentAccessToken, 'bzs_login');
        expect(
          h.store.data[origin],
          StoredTokenSession(refreshToken: 'bzr_login', principalId: 'p' * 64),
        );
        expect(
          h.launcher.opened.single.queryParameters['device_name'],
          'Test phone',
        );
        expect(
          h.states.map((s) => s.status),
          containsAllInOrder([
            TokenSessionStatus.signingIn,
            TokenSessionStatus.signedIn,
          ]),
        );
      },
    );

    test('cancel returns to signed out without an error', () async {
      final h = Harness();
      await h.controller.restore();
      h.launcher.error = const WebAuthCancelledException();
      expect(await h.controller.signIn(), isFalse);
      expect(h.status, TokenSessionStatus.signedOut);
      expect(h.controller.state.errorMessage, isNull);
    });

    test('a rejected login surfaces the reason', () async {
      final h = Harness();
      await h.controller.restore();
      h.launcher.respond = (start) =>
          Uri.parse('xyz.block.buzz://auth/cb').replace(
            queryParameters: {
              'error': 'principal_disabled',
              'state': start.queryParameters['state']!,
            },
          );
      expect(await h.controller.signIn(), isFalse);
      expect(h.status, TokenSessionStatus.signedOut);
      expect(h.controller.state.errorMessage, isNotNull);
    });

    test(
      'a failed persist revokes the new device session and stays signed out',
      () async {
        final h = Harness();
        await h.controller.restore();
        h.store.writeError = StateError('keychain unavailable');
        expect(await h.controller.signIn(), isFalse);
        expect(h.status, TokenSessionStatus.signedOut);
        expect(h.controller.currentAccessToken, isNull);
        expect(h.logouts.single.headers['Authorization'], 'Bearer bzs_login');
      },
    );

    test(
      'sign-out while the browser is open discards and revokes the late login',
      () async {
        final h = Harness();
        await h.controller.restore();
        h.launcher.gate = Completer<void>();
        final signIn = h.controller.signIn();
        await settle();
        expect(h.status, TokenSessionStatus.signingIn);
        await h.controller.signOut();
        h.launcher.gate!.complete();
        expect(await signIn, isFalse);
        await settle();
        expect(h.store.writes, 0, reason: 'stale login never persisted');
        expect(h.store.data, isEmpty);
        expect(h.status, TokenSessionStatus.signedOut);
        expect(h.controller.currentAccessToken, isNull);
        expect(h.clock.active, isEmpty);
        expect(h.logouts.single.headers['Authorization'], 'Bearer bzs_login');
      },
    );

    test(
      'sign-out while the login code is being exchanged discards the grant',
      () async {
        final h = Harness();
        await h.controller.restore();
        final exchange = Completer<http.Response>();
        h.server.heldComplete = exchange;
        final signIn = h.controller.signIn();
        await settle();
        expect(
          h.server.requests.where((r) => r.url.path == '/auth/oidc/complete'),
          hasLength(1),
        );
        await h.controller.signOut();
        exchange.complete(
          jsonResponse({
            'principal_id': 'p' * 64,
            'device_id': 'device-1',
            'access': 'bzs_login',
            'refresh': 'bzr_login',
            'expires_in': 3600,
          }),
        );
        expect(await signIn, isFalse);
        await settle();
        expect(h.store.writes, 0);
        expect(h.status, TokenSessionStatus.signedOut);
        expect(h.controller.currentAccessToken, isNull);
        expect(h.logouts.single.headers['Authorization'], 'Bearer bzs_login');
      },
    );

    test(
      'dispose while the browser is open never persists the login',
      () async {
        final h = Harness();
        await h.controller.restore();
        h.launcher.gate = Completer<void>();
        final signIn = h.controller.signIn();
        await settle();
        final before = h.states.length;
        h.controller.dispose();
        h.launcher.gate!.complete();
        expect(await signIn, isFalse);
        await settle();
        expect(h.store.writes, 0);
        expect(h.store.data, isEmpty);
        expect(h.controller.currentAccessToken, isNull);
        expect(h.states.length, before);
        expect(h.clock.active, isEmpty);
        expect(h.logouts.single.headers['Authorization'], 'Bearer bzs_login');
      },
    );
  });

  group('sign out', () {
    test(
      'revokes on the relay, deletes the refresh and cancels timers',
      () async {
        final h = Harness();
        await h.signedIn();
        await h.controller.signOut();
        expect(h.logouts.single.headers['Authorization'], 'Bearer bzs_2');
        expect(h.status, TokenSessionStatus.signedOut);
        expect(h.store.data, isEmpty);
        expect(h.controller.currentAccessToken, isNull);
        expect(h.clock.active, isEmpty);
      },
    );

    test('a network failure keeps the session and reports the error', () async {
      final h = Harness();
      await h.signedIn();
      h.server.logout = (_) => throw http.ClientException('offline');
      await expectLater(
        h.controller.signOut(),
        throwsA(isA<TokenSessionException>()),
      );
      expect(h.status, TokenSessionStatus.signedIn);
      expect(h.store.data[origin]!.refreshToken, 'bzr_2');
      expect(h.controller.currentAccessToken, 'bzs_2');
    });

    test(
      'an expired access token is refreshed once and logout retried',
      () async {
        final h = Harness();
        await h.signedIn();
        var calls = 0;
        h.server.logout = (_) => ++calls == 1
            ? FakeAuthServer.authError(401, 'token_expired')
            : http.Response('', 204);
        h.server.refreshResponses.add(
          (_) => FakeAuthServer.rotated('bzs_3', 'bzr_3'),
        );
        await h.controller.signOut();
        expect(h.logouts.last.headers['Authorization'], 'Bearer bzs_3');
        expect(h.status, TokenSessionStatus.signedOut);
        expect(h.store.data, isEmpty);
      },
    );

    test('a revoked device (401/404) is forgotten locally', () async {
      final h = Harness();
      await h.signedIn();
      h.server.logout = (_) => FakeAuthServer.authError(404, 'not_found');
      await h.controller.signOut();
      expect(h.status, TokenSessionStatus.signedOut);
      expect(h.store.data, isEmpty);
    });

    test(
      'a rotation completing after sign-out cannot resurrect the session',
      () async {
        final h = Harness();
        await h.signedIn();
        h.server.holdRefreshes = true;
        final late = h.controller.refreshAfterTokenExpired('bzs_2');
        await settle();
        expect(h.server.heldRefreshes.length, 1);
        await h.controller.signOut();
        expect(h.store.data, isEmpty);
        h.server.heldRefreshes.removeFirst().complete(
          FakeAuthServer.rotated('bzs_3', 'bzr_3'),
        );
        await late;
        await settle();
        expect(
          h.store.data,
          isEmpty,
          reason: 'stale write fenced by generation',
        );
        expect(h.status, TokenSessionStatus.signedOut);
        expect(h.controller.currentAccessToken, isNull);
        expect(h.clock.active, isEmpty);
      },
    );
  });

  test('dispose cancels timers and fences an in-flight rotation: a device-only '
      'removal (delete, then dispose) is never resurrected', () async {
    final h = Harness();
    await h.signedIn();
    h.server.holdRefreshes = true;
    unawaited(h.controller.refreshAfterTokenExpired('bzs_2'));
    await settle();
    final before = h.states.length;
    // community_token_session.dart: delete the record, then invalidate
    // (dispose) the controller.
    await h.store.delete(origin);
    h.controller.dispose();
    expect(h.clock.active, isEmpty);
    h.server.heldRefreshes.removeFirst().complete(
      FakeAuthServer.rotated('bzs_3', 'bzr_3'),
    );
    await settle();
    expect(
      h.store.data.containsKey(origin),
      isFalse,
      reason: 'a late rotation must not re-persist a removed session',
    );
    expect(h.states.length, before, reason: 'no notifications after dispose');
    expect(h.clock.active, isEmpty, reason: 'no timer re-armed after dispose');
  });
}
