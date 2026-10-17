import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:kuzz/shared/auth/account/account_api.dart';
import 'package:kuzz/shared/auth/token/token.dart';

import '../token/token_auth_test_fakes.dart';
import 'fake_account_server.dart';

const _origin = 'https://relay.test';
final _bob = 'b' * 64;
final _cat = 'c' * 64;

void main() {
  late FakeAccountServer server;
  late FakeRefreshTokenStore store;
  late ProviderContainer container;

  setUp(() {
    server = FakeAccountServer();
    store = FakeRefreshTokenStore();
    final clock = FakeClock(DateTime.utc(2026, 10, 18, 12));
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

  group('profile', () {
    test('discoverable defaults to true when /auth/me omits it', () async {
      final api = await signedIn();
      expect((await api.me()).discoverable, isTrue);

      server.profile = {...server.profile, 'discoverable': false};
      expect((await api.me()).discoverable, isFalse);
    });

    test('setUsername and setDiscoverable send only their field', () async {
      final api = await signedIn();
      final saved = await api.setUsername('ada_l');
      await api.setDiscoverable(false);

      final patches = server.calls('PATCH', '/auth/profile');
      expect(jsonDecode(patches[0].body), {'username': 'ada_l'});
      expect(jsonDecode(patches[1].body), {'discoverable': false});
      expect(saved.username, 'ada_l');
    });

    test('a taken ID keeps its 409 username_taken code', () async {
      final api = await signedIn();
      server.failures['/auth/profile'] = (_) => jsonResponse({
        'error': 'username already taken',
        'code': 'username_taken',
      }, status: 409);

      await expectLater(
        api.setUsername('bob'),
        throwsA(
          isA<AccountApiException>()
              .having((e) => e.statusCode, 'statusCode', 409)
              .having((e) => e.code, 'code', 'username_taken'),
        ),
      );
    });
  });

  group('blocks', () {
    test('block, list and unblock hit their endpoints', () async {
      final api = await signedIn();
      await api.block(_bob.toUpperCase());
      await api.block(_cat);

      final listed = await api.listBlocks();
      expect(listed.map((p) => p.pubkey), [_bob, _cat]);
      expect(listed.first.createdAt, isNotNull);

      await api.unblock(_bob);
      expect((await api.listBlocks()).map((p) => p.pubkey), [_cat]);
      expect(server.calls('PUT', '/auth/blocks/$_bob'), hasLength(1));
      expect(server.calls('DELETE', '/auth/blocks/$_bob'), hasLength(1));
    });

    test('a malformed key never reaches a request path', () async {
      final api = await signedIn();
      expect(() => api.block('../me'), throwsArgumentError);
      expect(server.accountRequests, isEmpty);
    });
  });

  group('people', () {
    test('lookup finds one person by exact ID or email', () async {
      final api = await signedIn();
      server.directory['bob'] = {
        'pubkey': _bob,
        'display_name': 'Bob',
        'avatar_url': null,
        'username': 'bob',
      };

      final found = await api.lookupPerson('  @Bob ');
      expect(found?.pubkey, _bob);
      expect(found?.displayName, 'Bob');
      expect(found?.username, 'bob');
      expect(
        server.calls('GET', '/auth/people/lookup').single.url.queryParameters,
        {'q': 'Bob'},
      );
    });

    test('lookup answers null on 404 and throws on 429', () async {
      final api = await signedIn();
      expect(await api.lookupPerson('nobody'), isNull);

      server.failures['/auth/people/lookup'] = (_) =>
          jsonResponse({'error': 'slow down'}, status: 429);
      await expectLater(
        api.lookupPerson('bob'),
        throwsA(
          isA<AccountApiException>().having(
            (e) => e.statusCode,
            'statusCode',
            429,
          ),
        ),
      );
    });

    test('shared people sends q and limit and skips malformed rows', () async {
      final api = await signedIn();
      server.sharedPeople = [
        {'pubkey': _bob, 'display_name': 'Bob', 'username': null},
        {'pubkey': 'not-a-key', 'display_name': 'Broken'},
        {'pubkey': _cat, 'display_name': 'Cat'},
      ];

      final people = await api.sharedPeople('b');
      expect(people.map((p) => p.pubkey), [_bob]);
      expect(
        server.calls('GET', '/auth/people/shared').single.url.queryParameters,
        {'q': 'b', 'limit': '50'},
      );
    });
  });
}
