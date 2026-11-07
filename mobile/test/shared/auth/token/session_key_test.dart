import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/shared/auth/token/token.dart';
import 'package:kuzz/shared/community/community.dart';

import '../../community/community_storage_test.dart' show FakeSecureStorage;
import 'token_auth_test_fakes.dart';

const _origin = 'https://relay.test';

void main() {
  test('equal by canonical origin and principal', () {
    expect(SessionKey('wss://Relay.Test/', 'a'), SessionKey(_origin, 'a'));
    expect(
      SessionKey('wss://Relay.Test/', 'a').hashCode,
      SessionKey(_origin, 'a').hashCode,
    );
    expect(SessionKey(_origin, 'a'), isNot(SessionKey(_origin, 'b')));
    expect(() => SessionKey('ftp://relay.test', 'a'), throwsFormatException);
  });

  group('of a community', () {
    test('a token community is its pubkey', () {
      final community = Community.create(
        name: 'T',
        relayUrl: _origin,
        pubkey: 'token-pubkey',
        googleBackupAccountId: 'ignored',
        tokenAuth: true,
      );
      expect(
        SessionKey.ofCommunity(community),
        SessionKey(_origin, 'token-pubkey'),
      );
    });

    test('a key community is its Google account', () {
      final community = Community.create(
        name: 'K',
        relayUrl: _origin,
        pubkey: 'key-pubkey',
        googleBackupAccountId: 'google-account',
      );
      expect(
        SessionKey.ofCommunity(community),
        SessionKey(_origin, 'google-account'),
      );
    });

    test('a key-only community has none', () {
      final community = Community.create(
        name: 'L',
        relayUrl: _origin,
        pubkey: 'key-pubkey',
      );
      expect(SessionKey.ofCommunity(community), isNull);
    });
  });

  test('two accounts on one origin keep separate sessions', () async {
    final a = SessionKey(_origin, 'a');
    final b = SessionKey(_origin, 'b');
    const first = StoredTokenSession(refreshToken: 'r1', principalId: 'a');
    const second = StoredTokenSession(refreshToken: 'r2', principalId: 'b');

    final store = SecureRefreshTokenStore(storage: FakeSecureStorage());
    await store.write(a, first);
    await store.write(b, second);
    expect(await store.read(a), first);
    expect(await store.read(b), second);
    await store.delete(a);
    expect(await store.read(a), isNull);
    expect(await store.read(b), second);

    final clock = FakeClock(DateTime.utc(2026, 1, 1));
    final server = FakeAuthServer();
    final fake = FakeRefreshTokenStore()
      ..data[a] = first
      ..data[b] = second;
    TokenSessionController controller(SessionKey key) => TokenSessionController(
      key: key,
      api: AuthApi(origin: _origin, client: server.client),
      store: fake,
      clock: clock.call,
      timerFactory: clock.createTimer,
    );
    final controllerA = controller(a);
    final controllerB = controller(b);
    addTearDown(controllerA.dispose);
    addTearDown(controllerB.dispose);
    server.refreshResponses
      ..add((_) => FakeAuthServer.rotated('bzs_a', 'bzr_a'))
      ..add((_) => FakeAuthServer.rotated('bzs_b', 'bzr_b'));
    await controllerA.restore();
    await controllerB.restore();

    expect(controllerA.currentAccessToken, 'bzs_a');
    expect(controllerB.currentAccessToken, 'bzs_b');
    expect(fake.data[a]?.refreshToken, 'bzr_a');
    expect(fake.data[b]?.refreshToken, 'bzr_b');

    server.logout = (_) => FakeAuthServer.authError(404, 'not_found');
    await controllerA.signOut();
    expect(controllerB.currentAccessToken, 'bzs_b');
    expect(fake.data[b]?.refreshToken, 'bzr_b');
  });
}
