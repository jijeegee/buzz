import 'dart:convert';
import 'dart:math';

import 'package:buzz/shared/auth/token/token.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';

import '../../community/community_storage_test.dart' show FakeSecureStorage;
import 'token_auth_test_fakes.dart';

const origin = 'https://relay.example';

void main() {
  group('AuthApi', () {
    test('startUri carries the mobile client contract', () {
      final api = AuthApi(
        origin: origin,
        client: MockClient((_) async => http.Response('', 500)),
      );
      final uri = api.startUri(
        provider: 'google',
        state: 's' * 32,
        codeChallenge: 'c' * 43,
        redirectUri: buzzMobileRedirectUri,
        deviceName: 'Pixel 9',
      );
      expect(uri.origin, origin);
      expect(uri.path, '/auth/oidc/google/start');
      expect(uri.queryParameters, {
        'state': 's' * 32,
        'code_challenge': 'c' * 43,
        'client': 'mobile',
        'identity_mode': 'token',
        'redirect_uri': 'xyz.block.buzz://auth/cb',
        'device_name': 'Pixel 9',
      });
    });

    test(
      'completeLogin posts the code and verifier and parses the grant',
      () async {
        final server = FakeAuthServer();
        final grant = await AuthApi(
          origin: origin,
          client: server.client,
        ).completeLogin(loginCode: 'bzl_x', codeVerifier: 'v' * 43);
        final request = server.requests.single;
        expect(request.method, 'POST');
        expect(request.url.toString(), '$origin/auth/oidc/complete');
        expect(jsonDecode(request.body), {
          'login_code': 'bzl_x',
          'code_verifier': 'v' * 43,
        });
        expect(grant.principalId, 'p' * 64);
        expect(grant.deviceId, 'device-1');
        expect(grant.accessToken, 'bzs_login');
        expect(grant.refreshToken, 'bzr_login');
        expect(grant.expiresIn, const Duration(hours: 1));
      },
    );

    test('refresh posts the token and classifies failures', () async {
      final server = FakeAuthServer()
        ..refreshResponses.addAll([
          (_) => FakeAuthServer.rotated('bzs_2', 'bzr_2', ttl: 600),
          (_) => FakeAuthServer.authError(401, 'refresh_reused'),
          (_) => FakeAuthServer.authError(400, 'invalid_request'),
          (_) => FakeAuthServer.authError(503, 'unavailable'),
          (_) => FakeAuthServer.authError(429, 'rate_limited'),
          (_) => jsonResponse({'access': 'only'}),
        ]);
      final api = AuthApi(origin: origin, client: server.client);
      final rotated = await api.refresh('bzr_1');
      expect(jsonDecode(server.requests.single.body), {'refresh': 'bzr_1'});
      expect(rotated.accessToken, 'bzs_2');
      expect(rotated.refreshToken, 'bzr_2');
      expect(rotated.expiresIn, const Duration(minutes: 10));

      Future<AuthApiException> failure() async {
        try {
          await api.refresh('bzr_1');
        } on AuthApiException catch (error) {
          return error;
        }
        fail('expected AuthApiException');
      }

      final reused = await failure();
      expect(
        (reused.kind, reused.statusCode, reused.code),
        (AuthFailureKind.terminal, 401, 'refresh_reused'),
      );
      expect((await failure()).kind, AuthFailureKind.terminal);
      expect((await failure()).kind, AuthFailureKind.transient);
      expect((await failure()).kind, AuthFailureKind.transient);
      final malformed = await failure();
      expect(
        malformed.kind,
        AuthFailureKind.transient,
        reason: 'missing refresh in a 200',
      );
    });

    test('network errors are transient', () async {
      final api = AuthApi(
        origin: origin,
        client: MockClient((_) async => throw http.ClientException('offline')),
      );
      await expectLater(
        api.refresh('bzr_1'),
        throwsA(
          isA<AuthApiException>().having(
            (e) => e.kind,
            'kind',
            AuthFailureKind.transient,
          ),
        ),
      );
    });

    test('logout sends the bearer access token', () async {
      final server = FakeAuthServer();
      await AuthApi(origin: origin, client: server.client).logout('bzs_a');
      final request = server.requests.single;
      expect(request.url.toString(), '$origin/auth/logout');
      expect(request.headers['Authorization'], 'Bearer bzs_a');
      server.logout = (_) => FakeAuthServer.authError(401, 'token_expired');
      await expectLater(
        AuthApi(origin: origin, client: server.client).logout('bzs_a'),
        throwsA(
          isA<AuthApiException>().having(
            (e) => e.code,
            'code',
            'token_expired',
          ),
        ),
      );
    });
  });

  group('runOidcLogin', () {
    test(
      'opens the start URL, validates state, exchanges the code with its verifier',
      () async {
        final server = FakeAuthServer();
        final launcher = FakeWebAuthLauncher(FakeWebAuthLauncher.success);
        final grant = await runOidcLogin(
          api: AuthApi(origin: origin, client: server.client),
          launcher: launcher,
          deviceName: 'Phone',
          random: Random(7),
        );
        expect(grant.refreshToken, 'bzr_login');
        expect(launcher.schemes.single, 'xyz.block.buzz');
        final start = launcher.opened.single;
        expect(start.path, '/auth/oidc/google/start');
        expect(start.queryParameters['client'], 'mobile');
        expect(start.queryParameters['redirect_uri'], buzzMobileRedirectUri);
        final body =
            jsonDecode(server.requests.single.body) as Map<String, dynamic>;
        expect(body['login_code'], 'bzl_code');
        expect(
          pkceChallenge(body['code_verifier'] as String),
          start.queryParameters['code_challenge'],
        );
      },
    );

    test('a forged state never reaches the code exchange', () async {
      final server = FakeAuthServer();
      final launcher = FakeWebAuthLauncher(
        (_) => Uri.parse(
          'xyz.block.buzz://auth/cb?code=bzl_evil&state=forged-state-value',
        ),
      );
      await expectLater(
        runOidcLogin(
          api: AuthApi(origin: origin, client: server.client),
          launcher: launcher,
        ),
        throwsA(
          isA<OidcLoginException>().having(
            (e) => e.code,
            'code',
            'state_mismatch',
          ),
        ),
      );
      expect(server.requests, isEmpty);
    });

    test('provider error callbacks surface their code', () async {
      final server = FakeAuthServer();
      final launcher = FakeWebAuthLauncher(
        (start) => Uri.parse('xyz.block.buzz://auth/cb').replace(
          queryParameters: {
            'error': 'principal_disabled',
            'state': start.queryParameters['state']!,
          },
        ),
      );
      await expectLater(
        runOidcLogin(
          api: AuthApi(origin: origin, client: server.client),
          launcher: launcher,
        ),
        throwsA(
          isA<OidcLoginException>().having(
            (e) => e.code,
            'code',
            'principal_disabled',
          ),
        ),
      );
      expect(server.requests, isEmpty);
    });

    test(
      'an error callback with a forged state is reported as a state mismatch',
      () async {
        final launcher = FakeWebAuthLauncher(
          (_) => Uri.parse(
            'xyz.block.buzz://auth/cb?error=access_denied&state=forged-state-value',
          ),
        );
        await expectLater(
          runOidcLogin(
            api: AuthApi(origin: origin, client: FakeAuthServer().client),
            launcher: launcher,
          ),
          throwsA(
            isA<OidcLoginException>().having(
              (e) => e.code,
              'code',
              'state_mismatch',
            ),
          ),
        );
      },
    );

    test('cancellation propagates', () async {
      final launcher = FakeWebAuthLauncher(FakeWebAuthLauncher.success)
        ..error = const WebAuthCancelledException();
      await expectLater(
        runOidcLogin(
          api: AuthApi(origin: origin, client: FakeAuthServer().client),
          launcher: launcher,
        ),
        throwsA(isA<WebAuthCancelledException>()),
      );
    });
  });

  group('SecureRefreshTokenStore', () {
    test('round-trips one record per origin and deletes it', () async {
      final secure = FakeSecureStorage();
      final store = SecureRefreshTokenStore(storage: secure);
      const a = StoredTokenSession(refreshToken: 'bzr_a', principalId: 'pa');
      const b = StoredTokenSession(refreshToken: 'bzr_b', principalId: 'pb');
      await store.write('https://a.example', a);
      await store.write('https://b.example', b);
      expect(await store.read('https://a.example'), a);
      expect(await store.read('https://b.example'), b);
      expect(
        await secure.read(
          key: SecureRefreshTokenStore.keyFor('https://a.example'),
        ),
        isNotNull,
      );
      await store.delete('https://a.example');
      expect(await store.read('https://a.example'), isNull);
      expect(await store.read('https://b.example'), b);
    });

    test('a corrupt record reads as absent and is removed', () async {
      final secure = FakeSecureStorage();
      await secure.write(
        key: SecureRefreshTokenStore.keyFor(origin),
        value: '{not json',
      );
      final store = SecureRefreshTokenStore(storage: secure);
      expect(await store.read(origin), isNull);
      expect(
        await secure.read(key: SecureRefreshTokenStore.keyFor(origin)),
        isNull,
      );
    });
  });
}
