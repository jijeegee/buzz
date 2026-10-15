import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart' as http_testing;
import 'package:nostr/nostr.dart' as nostr;

import 'package:buzz/features/invites/invite_join_provider.dart';
import 'package:buzz/shared/auth/auth.dart';
import 'package:buzz/shared/auth/default_community.dart';
import 'package:buzz/shared/deeplink/deep_link.dart';
import 'package:buzz/shared/deeplink/pending_deep_link_provider.dart';
import 'package:buzz/shared/relay/relay_provider.dart';

import '../community/community_storage_test.dart';

const _builtIn = 'https://jihobuzz.duckdns.org:8443';

Community _token(String name, String relayUrl) => Community.create(
  name: name,
  relayUrl: relayUrl,
  pubkey: nostr.Keys.generate().public,
  tokenAuth: true,
);

void main() {
  group('builtInServerOrigin (environment, overrides) -> origin', () {
    test('prod always uses its table origin and ignores dart-defines', () {
      expect(builtInServerOrigin(KuzzEnvironment.prod), _builtIn);
      expect(
        builtInServerOrigin(
          KuzzEnvironment.prod,
          keyBackupOriginDefine: 'http://localhost:3000',
          relayUrlDefine: 'wss://other.example.com',
        ),
        _builtIn,
      );
    });

    test('dev uses its table origin unless a dart-define overrides it', () {
      expect(builtInServerOrigin(KuzzEnvironment.dev), _builtIn);
      expect(
        builtInServerOrigin(
          KuzzEnvironment.dev,
          keyBackupOriginDefine: 'http://localhost:3000',
          relayUrlDefine: 'wss://other.example.com',
        ),
        'http://localhost:3000',
      );
      expect(
        builtInServerOrigin(
          KuzzEnvironment.dev,
          relayUrlDefine: 'wss://Other.Example.com/',
        ),
        'https://other.example.com',
      );
    });

    test('only the test harness is unrestricted', () {
      expect(
        builtInServerOrigin(KuzzEnvironment.prod, unrestrictedForTests: true),
        isNull,
      );
    });

    test('KUZZ_ENV defaults to prod; only "dev" selects dev', () {
      expect(parseKuzzEnvironment(''), KuzzEnvironment.prod);
      expect(parseKuzzEnvironment('prod'), KuzzEnvironment.prod);
      expect(parseKuzzEnvironment('staging'), KuzzEnvironment.prod);
      expect(parseKuzzEnvironment(' DEV '), KuzzEnvironment.dev);
    });

    test('the gate follows the origin provider', () {
      final container = ProviderContainer(
        overrides: [defaultCommunityOriginProvider.overrideWithValue(_builtIn)],
      );
      addTearDown(container.dispose);
      expect(container.read(hasBuiltInServerProvider), isTrue);
      final harness = ProviderContainer();
      addTearDown(harness.dispose);
      expect(harness.read(hasBuiltInServerProvider), isFalse);
    });
  });

  group('isBuiltInServerUrl compares host and port', () {
    test('ws/wss/http/https of the same host:port are the same server', () {
      for (final url in [
        'wss://jihobuzz.duckdns.org:8443',
        'ws://jihobuzz.duckdns.org:8443',
        'http://JIHOBUZZ.duckdns.org:8443/path',
        'https://jihobuzz.duckdns.org:8443',
      ]) {
        expect(isBuiltInServerUrl(url, _builtIn), isTrue, reason: url);
      }
    });

    test('another host, another port or a malformed URL is not', () {
      for (final url in [
        'wss://jihobuzz.duckdns.org',
        'wss://jihobuzz.duckdns.org:9443',
        'wss://evil.example.com:8443',
        'http://localhost:3000',
        'ftp://jihobuzz.duckdns.org:8443',
      ]) {
        expect(isBuiltInServerUrl(url, _builtIn), isFalse, reason: url);
      }
      expect(
        isServerAllowed('wss://evil.example.com', builtInOrigin: null),
        isTrue,
      );
      expect(
        isServerAllowed('wss://evil.example.com', builtInOrigin: _builtIn),
        isFalse,
      );
    });
  });

  group('startup pinning', () {
    final foreign = _token('Old', 'wss://old.example.com');
    final kuzzA = _token('Kuzz A', 'wss://jihobuzz.duckdns.org:8443');
    final kuzzB = _token('Kuzz B', 'https://jihobuzz.duckdns.org:8443');

    test('selectActiveCommunity prefers the built-in server', () {
      final all = [foreign, kuzzA, kuzzB];
      expect(
        selectActiveCommunity(all, foreign.id, builtInOrigin: _builtIn),
        kuzzA,
      );
      // Another account on the same server stays selected.
      expect(
        selectActiveCommunity(all, kuzzB.id, builtInOrigin: _builtIn),
        kuzzB,
      );
      expect(
        selectActiveCommunity([foreign], foreign.id, builtInOrigin: _builtIn),
        isNull,
      );
      // Harness: upstream rule.
      expect(
        selectActiveCommunity(all, foreign.id, builtInOrigin: null),
        foreign,
      );
      expect(
        selectActiveCommunity(all, 'missing', builtInOrigin: null),
        foreign,
      );
    });

    test('startup selects the built-in server and keeps the others', () async {
      final storage = CommunityStorage(secure: FakeSecureStorage());
      await storage.save(foreign);
      await storage.save(kuzzA);
      await storage.saveActiveId(foreign.id);
      final container = ProviderContainer(
        overrides: [
          communityStorageProvider.overrideWithValue(storage),
          defaultCommunityOriginProvider.overrideWithValue(_builtIn),
        ],
      );
      addTearDown(container.dispose);

      final auth = await container.read(authProvider.future);
      expect(auth.status, AuthStatus.authenticated);
      expect(auth.community?.id, kuzzA.id);
      expect(await storage.loadActiveId(), kuzzA.id);
      expect(
        (await container.read(activeCommunityProvider.future))?.id,
        kuzzA.id,
      );
      expect(
        (await storage.loadAll()).map((community) => community.id),
        containsAll([foreign.id, kuzzA.id]),
      );
    });

    test(
      'with only other servers stored, nothing is selected or deleted',
      () async {
        final storage = CommunityStorage(secure: FakeSecureStorage());
        await storage.save(foreign);
        await storage.saveActiveId(foreign.id);
        final container = ProviderContainer(
          overrides: [
            communityStorageProvider.overrideWithValue(storage),
            defaultCommunityOriginProvider.overrideWithValue(_builtIn),
          ],
        );
        addTearDown(container.dispose);

        final auth = await container.read(authProvider.future);
        expect(auth.status, AuthStatus.unauthenticated);
        expect(auth.community, isNull);
        expect(await container.read(activeCommunityProvider.future), isNull);
        expect(container.read(relayConfigProvider).baseUrl, _builtIn);
        expect((await storage.loadAll()).single.id, foreign.id);
      },
    );

    test('other servers cannot be signed in to or switched to', () async {
      final storage = CommunityStorage(secure: FakeSecureStorage());
      await storage.save(kuzzA);
      await storage.save(foreign);
      await storage.saveActiveId(kuzzA.id);
      final container = ProviderContainer(
        overrides: [
          communityStorageProvider.overrideWithValue(storage),
          defaultCommunityOriginProvider.overrideWithValue(_builtIn),
        ],
      );
      addTearDown(container.dispose);
      await container.read(authProvider.future);
      await container.read(communityListProvider.future);

      await expectLater(
        container
            .read(authProvider.notifier)
            .authenticateWithCommunity(_token('New', 'wss://new.example.com')),
        throwsStateError,
      );
      await expectLater(
        container
            .read(authProvider.notifier)
            .authenticateWithTokenSession(
              relayUrl: 'https://new.example.com',
              principalId: 'principal',
            ),
        throwsStateError,
      );
      await expectLater(
        container
            .read(communityListProvider.notifier)
            .switchCommunity(foreign.id),
        throwsStateError,
      );
      expect(await storage.loadActiveId(), kuzzA.id);
      expect((await storage.loadAll()).length, 2);
    });
  });

  group('invite host guard', () {
    ProviderContainer containerFor(
      CommunityStorage storage, {
      required void Function() onRequest,
    }) {
      final container = ProviderContainer(
        overrides: [
          communityStorageProvider.overrideWithValue(storage),
          defaultCommunityOriginProvider.overrideWithValue(_builtIn),
          inviteTokenAuthDetectorProvider.overrideWithValue((_) async {
            onRequest();
            return null;
          }),
          inviteJoinHttpClientProvider.overrideWithValue(
            http_testing.MockClient((_) async {
              onRequest();
              return http.Response('{}', 200);
            }),
          ),
        ],
      );
      addTearDown(container.dispose);
      return container;
    }

    test('an invite for another service adds and switches nothing', () async {
      final storage = CommunityStorage(secure: FakeSecureStorage());
      final kuzz = _token('Kuzz', 'wss://jihobuzz.duckdns.org:8443');
      await storage.save(kuzz);
      await storage.saveActiveId(kuzz.id);
      var requests = 0;
      final container = containerFor(storage, onRequest: () => requests++);
      final notifier = container.read(inviteJoinProvider.notifier);

      await notifier.prepare(
        const InviteDeepLink(relayUrl: 'wss://other.example.com', code: 'x'),
      );
      final state = container.read(inviteJoinProvider);
      expect(state.status, InviteJoinStatus.error);
      expect(state.differentService, isTrue);
      expect(state.errorMessage, 'This invite is for a different service.');
      expect(state.requiresFreshInvite, isTrue);

      await notifier.confirmJoin();
      expect(requests, 0);
      expect((await storage.loadAll()).single.id, kuzz.id);
      expect(await storage.loadActiveId(), kuzz.id);
    });

    test('an invite for the built-in server can be confirmed', () async {
      final storage = CommunityStorage(secure: FakeSecureStorage());
      var requests = 0;
      final container = containerFor(storage, onRequest: () => requests++);

      await container
          .read(inviteJoinProvider.notifier)
          .prepare(
            const InviteDeepLink(
              relayUrl: 'wss://jihobuzz.duckdns.org:8443',
              code: 'x',
            ),
          );
      final state = container.read(inviteJoinProvider);
      expect(state.status, InviteJoinStatus.confirming);
      expect(state.differentService, isFalse);
    });

    test('a notification for another server switches nothing', () async {
      PendingDeepLinkNotifier.debugUriStreamOverride = const Stream.empty();
      addTearDown(() => PendingDeepLinkNotifier.debugUriStreamOverride = null);
      final storage = CommunityStorage(secure: FakeSecureStorage());
      final kuzz = _token('Kuzz', 'wss://jihobuzz.duckdns.org:8443');
      final foreign = _token('Old', 'wss://old.example.com');
      await storage.save(kuzz);
      await storage.save(foreign);
      await storage.saveActiveId(kuzz.id);
      final container = ProviderContainer(
        overrides: [
          communityStorageProvider.overrideWithValue(storage),
          defaultCommunityOriginProvider.overrideWithValue(_builtIn),
        ],
      );
      addTearDown(container.dispose);

      final preparation = await container
          .read(pendingDeepLinkProvider.notifier)
          .prepareCommunity(
            MessageDeepLink(
              communityId: foreign.id,
              channelId: 'channel',
              messageId: 'message',
            ),
          );
      expect(preparation, DeepLinkCommunityPreparation.unavailable);
      expect(await storage.loadActiveId(), kuzz.id);
    });
  });
}
