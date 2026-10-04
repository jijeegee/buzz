import 'dart:convert';

import 'package:buzz/features/invites/invite_join_provider.dart';
import 'package:buzz/shared/auth/auth.dart';
import 'package:buzz/shared/auth/token/token.dart';
import 'package:buzz/shared/deeplink/deep_link.dart';
import 'package:buzz/shared/relay/relay.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart' as http_testing;
import 'package:nostr/nostr.dart' as nostr;

import '../../shared/auth/token/token_auth_test_fakes.dart';
import '../../shared/community/community_storage_test.dart';
import '../../shared/relay/fake_relay_access_tokens.dart';

const _principal =
    'bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22';

void main() {
  late CommunityStorage storage;
  late List<http.Request> requests;
  late List<http.Response> responses;
  late FakeRelayAccessTokens tokens;
  late List<String> tokenOrigins;
  var generatedKeys = 0;

  late _RecordingSession session;

  ProviderContainer build() {
    final container = ProviderContainer(
      overrides: [
        relaySessionProvider.overrideWith(() => session),
        communityStorageProvider.overrideWithValue(storage),
        inviteKeyGeneratorProvider.overrideWithValue(() {
          generatedKeys++;
          return nostr.Keys.generate();
        }),
        inviteClaimAccessTokensProvider.overrideWith((ref, origin) {
          tokenOrigins.add(origin);
          return tokens;
        }),
        inviteJoinHttpClientProvider.overrideWithValue(
          http_testing.MockClient((request) async {
            requests.add(request);
            return responses.removeAt(0);
          }),
        ),
      ],
    );
    addTearDown(container.dispose);
    return container;
  }

  setUp(() async {
    session = _RecordingSession();
    generatedKeys = 0;
    requests = [];
    responses = [];
    tokenOrigins = [];
    tokens = FakeRelayAccessTokens(token: 'bzs_1', rotations: ['bzs_2']);
    storage = CommunityStorage(secure: FakeSecureStorage());
    await storage.save(
      Community(
        id: 'token-id',
        name: 'Token relay',
        relayUrl: 'wss://relay.example.com',
        pubkey: _principal,
        tokenAuth: true,
        addedAt: DateTime.utc(2026),
      ),
    );
  });

  const invite = InviteDeepLink(
    relayUrl: 'wss://relay.example.com',
    code: 'v2.secret',
    policyReceipt: 'receipt',
  );

  http.Response claimed() => http.Response(
    jsonEncode({
      'status': 'joined',
      'community_id': 'c',
      'host': 'relay.example.com',
      'role': 'member',
    }),
    200,
  );

  test(
    'an existing token community claims membership with its bearer',
    () async {
      responses.add(claimed());
      final container = build();
      await container.read(communityListProvider.future);
      final notifier = container.read(inviteJoinProvider.notifier);

      await notifier.prepare(invite);
      // A token community may not be a member yet: the invite must be
      // claimed for the signed-in principal, not skipped.
      expect(
        container.read(inviteJoinProvider).status,
        InviteJoinStatus.confirming,
      );
      expect(requests, isEmpty);

      await notifier.confirmJoin();

      final state = container.read(inviteJoinProvider);
      expect(state.status, InviteJoinStatus.switchedExisting);
      expect(state.communityName, 'Token relay');
      expect(tokenOrigins.toSet(), {'https://relay.example.com'});
      expect(requests.single.headers['Authorization'], 'Bearer bzs_1');
      expect(
        requests.single.url,
        Uri.parse('https://relay.example.com/api/invites/claim'),
      );
      expect(jsonDecode(requests.single.body), {
        'code': 'v2.secret',
        'policy_receipt': 'receipt',
      });
      expect(generatedKeys, 0);
      final stored = (await storage.loadAll()).single;
      expect(stored.nsec, isNull);
      expect(stored.pubkey, _principal);
      expect(await storage.loadActiveId(), 'token-id');
    },
  );

  test('a token_expired claim rotates once and retries', () async {
    responses
      ..add(http.Response(jsonEncode({'error': 'token_expired'}), 401))
      ..add(claimed());
    final container = build();
    await container.read(communityListProvider.future);
    final notifier = container.read(inviteJoinProvider.notifier);

    await notifier.prepare(invite);
    await notifier.confirmJoin();

    expect(
      container.read(inviteJoinProvider).status,
      InviteJoinStatus.switchedExisting,
    );
    expect(tokens.expiredCalls, ['bzs_1']);
    expect(requests.map((r) => r.headers['Authorization']), [
      'Bearer bzs_1',
      'Bearer bzs_2',
    ]);
  });

  test('a signed-out token community asks to sign in, never keygens', () async {
    tokens.token = null;
    final container = build();
    await container.read(communityListProvider.future);
    final notifier = container.read(inviteJoinProvider.notifier);

    await notifier.prepare(invite);
    await notifier.confirmJoin();

    final state = container.read(inviteJoinProvider);
    expect(state.status, InviteJoinStatus.error);
    expect(state.errorMessage, 'Sign in to Token relay to use this invite.');
    expect(requests, isEmpty);
    expect(generatedKeys, 0);
  });

  test('claiming on the already-active token community reconnects its parked '
      'relay session (member-restricted park)', () async {
    await storage.saveActiveId('token-id');
    responses.add(claimed());
    final container = build();
    await container.read(communityListProvider.future);
    // The live session exists (parked on "restricted: not a relay member").
    container.read(relaySessionProvider);
    final notifier = container.read(inviteJoinProvider.notifier);

    await notifier.prepare(invite);
    await notifier.confirmJoin();

    expect(
      container.read(inviteJoinProvider).status,
      InviteJoinStatus.switchedExisting,
    );
    expect(session.reconnects, 1);
  });

  group('a new-community invite on a token relay', () {
    late FakeAuthServer server;
    late FakeWebAuthLauncher launcher;
    late FakeRefreshTokenStore store;
    late List<String> recoveryNsecs;
    const newInvite = InviteDeepLink(
      relayUrl: 'wss://new.example.com',
      code: 'v2.secret',
    );

    ProviderContainer buildNew() {
      final clock = FakeClock(DateTime.utc(2026, 1, 1));
      final container = ProviderContainer(
        overrides: [
          relaySessionProvider.overrideWith(() => session),
          communityStorageProvider.overrideWithValue(storage),
          communitySnapshotWriterProvider.overrideWithValue((_) async {}),
          inviteKeyGeneratorProvider.overrideWithValue(() {
            generatedKeys++;
            return nostr.Keys.generate();
          }),
          authHttpClientProvider.overrideWithValue(server.client),
          refreshTokenStoreProvider.overrideWithValue(store),
          webAuthLauncherProvider.overrideWithValue(launcher),
          sessionClockProvider.overrideWithValue(clock.call),
          sessionTimerFactoryProvider.overrideWithValue(clock.createTimer),
          inviteJoinRecoveryProvider.overrideWithValue((scope) {
            recoveryNsecs.add(scope.nsec ?? 'none');
            return const _Recovery();
          }),
          inviteJoinHttpClientProvider.overrideWithValue(
            http_testing.MockClient((request) async {
              requests.add(request);
              return responses.removeAt(0);
            }),
          ),
        ],
      );
      addTearDown(container.dispose);
      return container;
    }

    setUp(() async {
      server = FakeAuthServer();
      launcher = FakeWebAuthLauncher(FakeWebAuthLauncher.success);
      store = FakeRefreshTokenStore();
      recoveryNsecs = [];
      storage = CommunityStorage(secure: FakeSecureStorage());
    });

    test('signs in first, claims with the bearer, saves a token '
        'community, never generates a key', () async {
      responses.add(
        http.Response(
          jsonEncode({'status': 'joined', 'host': 'new.example.com'}),
          200,
        ),
      );
      final container = buildNew();
      await container.read(authProvider.future);
      final notifier = container.read(inviteJoinProvider.notifier);

      await notifier.prepare(newInvite);
      expect(
        container.read(inviteJoinProvider).tokenRelayOrigin,
        'https://new.example.com',
      );
      expect(launcher.opened, isEmpty, reason: 'no browser before consent');

      await notifier.confirmJoin();

      expect(launcher.opened, hasLength(1));
      expect(requests.single.headers['Authorization'], 'Bearer bzs_login');
      expect(generatedKeys, 0);
      final stored = (await storage.loadAll()).single;
      expect(stored.tokenAuth, isTrue);
      expect(stored.nsec, isNull);
      expect(stored.pubkey, 'p' * 64);
      expect(stored.relayUrl, 'wss://new.example.com');
      expect(
        container.read(inviteJoinProvider).status,
        InviteJoinStatus.success,
      );
      expect(recoveryNsecs, ['none']);
    });

    test('a cancelled sign-in never claims and never keygens', () async {
      launcher.error = const WebAuthCancelledException();
      final container = buildNew();
      await container.read(authProvider.future);
      final notifier = container.read(inviteJoinProvider.notifier);

      await notifier.prepare(newInvite);
      await notifier.confirmJoin();

      final state = container.read(inviteJoinProvider);
      expect(state.status, InviteJoinStatus.error);
      expect(state.errorMessage, startsWith('Sign in to '));
      expect(requests, isEmpty);
      expect(generatedKeys, 0);
      expect(await storage.loadAll(), isEmpty);
    });

    test('a NIP-11 outage never falls back to a legacy keypair', () async {
      server.nip11 = (_) => http.Response('down', 503);
      final container = buildNew();
      await container.read(authProvider.future);
      final notifier = container.read(inviteJoinProvider.notifier);

      await notifier.prepare(newInvite);
      await notifier.confirmJoin();

      expect(container.read(inviteJoinProvider).status, InviteJoinStatus.error);
      expect(requests, isEmpty);
      expect(generatedKeys, 0);
    });
  });
}

class _RecordingSession extends RelaySessionNotifier {
  int reconnects = 0;

  @override
  SessionState build() =>
      const SessionState(status: SessionStatus.disconnected);

  @override
  Future<void> reconnect() async => reconnects++;
}

class _Recovery implements InviteJoinRecovery {
  const _Recovery();

  @override
  Future<String?> ensureStarterChannels() async => null;
}
