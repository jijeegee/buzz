import 'dart:async';

import 'package:buzz/shared/auth/token/token.dart';
import 'package:buzz/shared/auth/google_key_backup.dart';
import 'package:buzz/shared/community/community.dart';
import 'package:buzz/shared/community/community_provider.dart';
import 'package:buzz/shared/community/community_storage.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:nostr/nostr.dart' as nostr;

import '../auth/token/token_auth_test_fakes.dart';
import 'community_storage_test.dart';

const _origin = 'https://relay.test';
final _principal = 'p' * 64;

/// Removing a token community must revoke this device's relay session before
/// any local state is erased (AGENTS.md rule 1).
void main() {
  late FakeAuthServer server;
  late FakeRefreshTokenStore tokens;
  late FakeRefreshTokenStore backupTokens;
  late PendingBackupKeyStore pending;
  late CommunityStorage storage;
  late ProviderContainer container;
  late List<String> journaled;

  setUp(() {
    server = FakeAuthServer();
    tokens = FakeRefreshTokenStore();
    backupTokens = FakeRefreshTokenStore();
    pending = PendingBackupKeyStore(storage: FakeSecureStorage());
    storage = CommunityStorage(secure: FakeSecureStorage());
    journaled = [];
    final clock = FakeClock(DateTime.utc(2026, 10, 5, 12));
    container = ProviderContainer(
      overrides: [
        communityStorageProvider.overrideWithValue(storage),
        communitySnapshotWriterProvider.overrideWithValue((_) async {}),
        communityPushLeaseRevocationEnqueuerProvider.overrideWithValue((
          community,
        ) async {
          journaled.add(community.id);
          return false;
        }),
        communityPushLeaseRevocationTriggerProvider.overrideWithValue(
          () async {},
        ),
        authHttpClientProvider.overrideWithValue(server.client),
        refreshTokenStoreProvider.overrideWithValue(tokens),
        keyBackupRefreshTokenStoreProvider.overrideWithValue(backupTokens),
        pendingBackupKeyStoreProvider.overrideWithValue(pending),
        webAuthLauncherProvider.overrideWithValue(
          FakeWebAuthLauncher(FakeWebAuthLauncher.success),
        ),
        sessionClockProvider.overrideWithValue(clock.call),
        sessionTimerFactoryProvider.overrideWithValue(clock.createTimer),
        authDeviceNameProvider.overrideWithValue('Test phone'),
      ],
    );
  });

  tearDown(() => container.dispose());

  Future<Community> addTokenCommunity() async {
    final community = Community.create(
      name: 'Relay',
      relayUrl: _origin,
      pubkey: _principal,
      tokenAuth: true,
    );
    await container.read(communityListProvider.future);
    await container
        .read(communityListProvider.notifier)
        .addCommunity(community);
    tokens.data[_origin] = StoredTokenSession(
      refreshToken: 'bzr_old',
      principalId: _principal,
    );
    return community;
  }

  Iterable<String> paths() => server.requests.map((r) => r.url.path);

  test(
    'custody logout removes pending local key without erasing token-mode session',
    () async {
      final keys = nostr.Keys.generate();
      final community = Community.create(
        name: 'Signed',
        relayUrl: _origin,
        pubkey: keys.public,
        nsec: keys.nsec,
        googleBackupAccountId: _principal,
      );
      await container.read(communityListProvider.future);
      await container
          .read(communityListProvider.notifier)
          .addCommunity(community);
      backupTokens.data[_origin] = StoredTokenSession(
        refreshToken: 'bzr_backup',
        principalId: _principal,
        identityMode: 'key_backup',
      );
      tokens.data[_origin] = const StoredTokenSession(
        refreshToken: 'untouched',
        principalId: 'old-token',
      );
      await pending.write(_origin, _principal, keys.nsec);
      server.refreshResponses.add(
        (_) => FakeAuthServer.rotated('bzs_backup', 'bzr_new'),
      );
      await container
          .read(communityListProvider.notifier)
          .removeCommunity(community.id);
      expect(await pending.read(_origin, _principal), isNull);
      expect(backupTokens.data, isEmpty);
      expect(tokens.data[_origin]!.refreshToken, 'untouched');
      expect(await storage.loadAll(), isEmpty);
    },
  );

  test('revokes the device session on the relay, then removes', () async {
    final community = await addTokenCommunity();
    server.refreshResponses.add(
      (_) => FakeAuthServer.rotated('bzs_new', 'bzr_new'),
    );

    await container
        .read(communityListProvider.notifier)
        .removeCommunity(community.id);

    final logout = server.requests.singleWhere(
      (r) => r.url.path == '/auth/logout',
    );
    expect(logout.headers['authorization'], 'Bearer bzs_new');
    expect(tokens.data, isEmpty);
    expect(await storage.loadAll(), isEmpty);
    expect(journaled, [community.id]);
  });

  test('an unreachable relay keeps the community and its session', () async {
    final community = await addTokenCommunity();
    for (var i = 0; i < 3; i++) {
      server.refreshResponses.add(
        (_) => jsonResponse({'error': 'unavailable'}, status: 503),
      );
    }

    await expectLater(
      container
          .read(communityListProvider.notifier)
          .removeCommunity(community.id),
      throwsA(isA<TokenSessionException>()),
    );

    expect(paths(), isNot(contains('/auth/logout')));
    expect(tokens.data[_origin]?.refreshToken, 'bzr_old');
    expect((await storage.loadAll()).map((c) => c.id), [community.id]);
    expect(journaled, isEmpty);
  });

  test('a failed relay logout keeps the community and its session', () async {
    final community = await addTokenCommunity();
    server.refreshResponses.add(
      (_) => FakeAuthServer.rotated('bzs_new', 'bzr_new'),
    );
    server.logout = (_) => jsonResponse({'error': 'down'}, status: 503);

    await expectLater(
      container
          .read(communityListProvider.notifier)
          .removeCommunity(community.id),
      throwsA(isA<TokenSessionException>()),
    );

    expect(tokens.data[_origin]?.refreshToken, 'bzr_new');
    expect((await storage.loadAll()).map((c) => c.id), [community.id]);
  });

  test('device-only removal forgets the session without the relay', () async {
    final community = await addTokenCommunity();

    await container
        .read(communityListProvider.notifier)
        .removeCommunity(community.id, deviceOnly: true);

    expect(server.requests, isEmpty);
    expect(tokens.data, isEmpty);
    expect(await storage.loadAll(), isEmpty);
    expect(
      container.read(tokenSessionControllerProvider(_origin)).state.status,
      TokenSessionStatus.restoring,
      reason: 'a fresh controller replaces the forgotten one',
    );
  });

  test(
    'device-only removal cannot be undone by an in-flight rotation',
    () async {
      final community = await addTokenCommunity();
      server.refreshResponses.add(
        (_) => FakeAuthServer.rotated('bzs_new', 'bzr_new'),
      );
      final gate = tokens.writeGate = Completer<void>();
      final restoring = container
          .read(tokenSessionControllerProvider(_origin))
          .restore();
      await settle();
      expect(server.refreshCalls, 1, reason: 'rotation write is in flight');

      final removing = container
          .read(communityListProvider.notifier)
          .removeCommunity(community.id, deviceOnly: true);
      await settle();
      gate.complete();
      await removing;
      await restoring;

      expect(tokens.data, isEmpty);
      expect(await storage.loadAll(), isEmpty);
    },
  );

  test('legacy communities are removed without auth requests', () async {
    await container.read(communityListProvider.future);
    final legacy = Community.create(name: 'Legacy', relayUrl: _origin);
    await container.read(communityListProvider.notifier).addCommunity(legacy);

    await container
        .read(communityListProvider.notifier)
        .removeCommunity(legacy.id);

    expect(server.requests, isEmpty);
    expect(await storage.loadAll(), isEmpty);
  });
}
