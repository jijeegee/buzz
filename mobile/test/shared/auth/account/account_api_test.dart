import 'dart:convert';

import 'package:buzz/shared/auth/account/account_api.dart';
import 'package:buzz/shared/auth/token/token.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../token/token_auth_test_fakes.dart';
import 'fake_account_server.dart';

const _origin = 'https://relay.test';

void main() {
  late FakeAccountServer server;
  late FakeRefreshTokenStore store;
  late ProviderContainer container;

  setUp(() {
    server = FakeAccountServer();
    store = FakeRefreshTokenStore();
    final clock = FakeClock(DateTime.utc(2026, 10, 5, 12));
    container = ProviderContainer(
      overrides: [
        authHttpClientProvider.overrideWithValue(server.client),
        refreshTokenStoreProvider.overrideWithValue(store),
        webAuthLauncherProvider.overrideWithValue(
          FakeWebAuthLauncher(FakeWebAuthLauncher.success),
        ),
        sessionClockProvider.overrideWithValue(clock.call),
        sessionTimerFactoryProvider.overrideWithValue(clock.createTimer),
        authDeviceNameProvider.overrideWithValue('Test phone'),
      ],
    );
    addTearDown(container.dispose);
  });

  Future<AccountApi> signedIn() async {
    store.data[_origin] = StoredTokenSession(
      refreshToken: 'bzr_old',
      principalId: 'p' * 64,
    );
    server.auth.refreshResponses.add(
      (_) => FakeAuthServer.rotated('bzs_1', 'bzr_1'),
    );
    await container.read(tokenSessionControllerProvider(_origin)).restore();
    return container.read(accountApiProvider(_origin));
  }

  test('me() reads the profile with the session Bearer token', () async {
    final api = await signedIn();
    final profile = await api.me();

    expect(profile.principalId, 'p' * 64);
    expect(profile.displayName, 'Ada');
    expect(profile.username, 'ada');
    expect(profile.avatarUrl, isNull);
    expect(
      server.calls('GET', '/auth/me').single.headers['Authorization'],
      'Bearer bzs_1',
    );
  });

  test('a profile save is one PATCH carrying every field', () async {
    final api = await signedIn();
    final saved = await api.updateProfile(
      displayName: 'Ada L',
      avatarUrl: 'https://cdn.test/a.png',
      username: null,
    );

    final patch = server.calls('PATCH', '/auth/profile').single;
    expect(jsonDecode(patch.body), {
      'display_name': 'Ada L',
      'avatar_url': 'https://cdn.test/a.png',
      'username': null,
    });
    expect(saved.displayName, 'Ada L');
    expect(saved.avatarUrl, 'https://cdn.test/a.png');
    expect(saved.username, isNull);
  });

  test('token_expired rotates once and retries with the new token', () async {
    final api = await signedIn();
    server.failures['/auth/devices'] = (_) =>
        FakeAuthServer.authError(401, 'token_expired');
    server.auth.refreshResponses.add(
      (_) => FakeAuthServer.rotated('bzs_2', 'bzr_2'),
    );

    final devices = await api.listDevices();

    expect(devices.map((d) => d.id), ['device-1', 'device-2', 'device-3']);
    expect(devices.first.current, isTrue);
    expect(
      server
          .calls('GET', '/auth/devices')
          .map((r) => r.headers['Authorization']),
      ['Bearer bzs_1', 'Bearer bzs_2'],
    );
  });

  test('relay errors keep their code (username_taken)', () async {
    final api = await signedIn();
    server.failures['/auth/profile'] = (_) => jsonResponse({
      'error': 'username already taken',
      'code': 'username_taken',
    }, status: 409);

    await expectLater(
      api.updateProfile(displayName: 'Ada', avatarUrl: null, username: 'bob'),
      throwsA(
        isA<AccountApiException>()
            .having((e) => e.statusCode, 'statusCode', 409)
            .having((e) => e.code, 'code', 'username_taken')
            .having((e) => e.message, 'message', 'username already taken'),
      ),
    );
  });

  test(
    'signed out: fails without sending an unauthenticated request',
    () async {
      final api = container.read(accountApiProvider(_origin));
      await container.read(tokenSessionControllerProvider(_origin)).restore();

      await expectLater(api.me(), throwsA(isA<AccountApiException>()));
      expect(server.accountRequests, isEmpty);
    },
  );

  test('device actions hit their endpoints', () async {
    final api = await signedIn();
    await api.revokeDevice('device-2');
    await api.revokeOtherSessions();
    await api.revokeAllBotTokens();
    await api.deleteAccount();

    expect(server.accountRequests.map((r) => '${r.method} ${r.url.path}'), [
      'DELETE /auth/devices/device-2',
      'POST /auth/sessions/revoke-others',
      'POST /auth/bots/revoke-all',
      'DELETE /auth/account',
    ]);
  });

  for (final (status, code) in [
    (401, 'token_revoked'),
    (403, 'principal_disabled'),
  ]) {
    test('$code ends the session so the gate shows sign-in', () async {
      final api = await signedIn();
      server.failures['/auth/me'] = (_) =>
          FakeAuthServer.authError(status, code);
      server.auth.refreshResponses.add(
        (_) => FakeAuthServer.authError(401, code),
      );

      await expectLater(
        api.me(),
        throwsA(isA<AccountApiException>().having((e) => e.code, 'code', code)),
      );

      expect(
        container.read(tokenSessionProvider(_origin)).status,
        TokenSessionStatus.signedOut,
      );
      expect(store.data, isEmpty);
    });
  }
}
