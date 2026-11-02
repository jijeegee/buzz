import 'package:kuzz/shared/push/dev_push_lease.dart';
import 'package:kuzz/shared/community/community.dart';
import 'package:kuzz/shared/community/community_provider.dart';
import 'package:kuzz/shared/push/push_bootstrap.dart';
import 'package:kuzz/shared/push/push_bridge.dart';
import 'package:kuzz/shared/push/push_lease_revocation_outbox.dart';
import 'package:kuzz/shared/push/push_relay_capability_provider.dart';
import 'package:kuzz/shared/push/push_subscription.dart';
import 'package:kuzz/shared/relay/relay_provider.dart' show Env;
import 'package:kuzz/shared/relay/relay_session.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:flutter/widgets.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:nostr/nostr.dart' as nostr;

import '../community/community_storage_test.dart' show FakeSecureStorage;

void main() {
  testWidgets('first opt-in starts registration without switching community', (
    tester,
  ) async {
    var registrations = 0;
    Widget bootstrap(bool enabled) => KuzzPushRegistrationBootstrap(
      shouldRegister: enabled,
      attemptKey: 'same-community|same-relay',
      startRegistration: () async {
        registrations += 1;
      },
      child: const SizedBox(),
    );
    await tester.pumpWidget(bootstrap(false));
    expect(registrations, 0);
    await tester.pumpWidget(bootstrap(true));
    expect(registrations, 1);
    await tester.pumpWidget(bootstrap(true));
    expect(registrations, 1);
  });

  test(
    'superseded lease acceptance is a retryable publication failure',
    () async {
      await expectLater(
        publishKuzzPushLeaseRecoverably(
          reserveGeneration: () async => 1,
          publish: (_) async {},
          markAccepted: (_) async => false,
        ),
        throwsStateError,
      );
    },
  );

  test('failed bootstrap attempt becomes retryable after the delay', () async {
    final gate = KuzzPushAttemptGate(retryDelay: Duration.zero);
    addTearDown(gate.dispose);
    var retries = 0;

    expect(gate.tryBegin('attempt'), isTrue);
    gate.failed('attempt', retry: () => retries += 1);
    await Future<void>.delayed(Duration.zero);

    expect(retries, 1);
    expect(gate.tryBegin('attempt'), isTrue);
  });

  test('a new attempt cancels an obsolete scheduled retry', () async {
    final gate = KuzzPushAttemptGate(retryDelay: Duration.zero);
    addTearDown(gate.dispose);
    var retries = 0;

    expect(gate.tryBegin('old'), isTrue);
    gate.failed('old', retry: () => retries += 1);
    expect(gate.tryBegin('new'), isTrue);
    await Future<void>.delayed(Duration.zero);

    expect(retries, 0);
    expect(gate.tryBegin('new'), isFalse);
  });

  test('successful bootstrap becomes retryable at renewal time', () async {
    final gate = KuzzPushAttemptGate(retryDelay: Duration.zero);
    addTearDown(gate.dispose);
    var retries = 0;

    expect(gate.tryBegin('attempt'), isTrue);
    gate.retryAfter('attempt', delay: Duration.zero, retry: () => retries += 1);
    await Future<void>.delayed(Duration.zero);

    expect(retries, 1);
    expect(gate.tryBegin('attempt'), isTrue);
  });

  test('repeated failures back off up to ten minutes', () {
    final gate = KuzzPushAttemptGate();
    addTearDown(gate.dispose);
    final delays = <int>[];
    for (var i = 0; i < 10; i++) {
      gate.retryNow();
      expect(gate.tryBegin('attempt'), isTrue);
      gate.failed('attempt', retry: () {});
      delays.add(gate.lastRetryDelay!.inSeconds);
    }
    expect(delays, [5, 10, 20, 40, 80, 160, 320, 600, 600, 600]);
  });

  test('a failed attempt is not restarted while it waits out its backoff', () {
    final gate = KuzzPushAttemptGate();
    addTearDown(gate.dispose);

    expect(gate.tryBegin('attempt'), isTrue);
    gate.failed('attempt', retry: () {});
    // Rebuilt inputs that produce the same attempt change nothing.
    expect(gate.tryBegin('attempt'), isFalse);
    expect(gate.waitingOutBackoff('attempt'), isTrue);

    // An explicit retry may start it at once.
    gate.retryNow();
    expect(gate.tryBegin('attempt'), isTrue);
    // A running attempt is not affected by an explicit retry.
    gate.retryNow();
    expect(gate.tryBegin('attempt'), isFalse);
  });

  test('success or a different attempt restarts the backoff', () {
    final gate = KuzzPushAttemptGate();
    addTearDown(gate.dispose);
    for (var i = 0; i < 3; i++) {
      gate.retryNow();
      gate.tryBegin('attempt');
      gate.failed('attempt', retry: () {});
    }
    expect(gate.lastRetryDelay, const Duration(seconds: 20));

    gate.retryNow();
    gate.tryBegin('attempt');
    gate.retryAfter('attempt', delay: const Duration(hours: 1), retry: () {});
    gate.complete('attempt');
    gate.tryBegin('attempt');
    gate.failed('attempt', retry: () {});
    expect(gate.lastRetryDelay, const Duration(seconds: 5));

    gate.retryNow();
    gate.tryBegin('attempt');
    gate.failed('attempt', retry: () {});
    gate.tryBegin('other');
    gate.failed('other', retry: () {});
    expect(gate.lastRetryDelay, const Duration(seconds: 5));
  });

  testWidgets('a failed registration retries with backoff and on resume', (
    tester,
  ) async {
    var registrations = 0;
    var fail = true;
    Widget bootstrap(int retrySignal) => KuzzPushRegistrationBootstrap(
      shouldRegister: true,
      attemptKey: 'community|relay',
      retrySignal: retrySignal,
      startRegistration: () async {
        registrations += 1;
        if (fail) throw StateError('FCM token unavailable');
      },
      child: const SizedBox(),
    );
    addTearDown(() => pushSetupError.value = null);

    await tester.pumpWidget(bootstrap(0));
    await tester.pump();
    expect(registrations, 1);
    expect(pushSetupError.value?.message, contains('FCM token unavailable'));

    await tester.pump(const Duration(seconds: 5));
    await tester.pump();
    expect(registrations, 2);
    await tester.pump(const Duration(seconds: 9));
    expect(registrations, 2);
    await tester.pump(const Duration(seconds: 1));
    await tester.pump();
    expect(registrations, 3);

    // Resume retries at once instead of waiting 20 s.
    fail = false;
    await tester.pumpWidget(bootstrap(1));
    await tester.pump();
    expect(registrations, 4);
    // A held successful registration is not repeated on the next resume.
    await tester.pumpWidget(bootstrap(2));
    await tester.pump();
    expect(registrations, 4);
  });

  testWidgets('a reconnect flip does not skip the registration backoff', (
    tester,
  ) async {
    var registrations = 0;
    Widget bootstrap(bool shouldRegister) => KuzzPushRegistrationBootstrap(
      shouldRegister: shouldRegister,
      attemptKey: 'community|relay',
      startRegistration: () async {
        registrations += 1;
        throw StateError('FCM token unavailable');
      },
      child: const SizedBox(),
    );
    addTearDown(() => pushSetupError.value = null);

    await tester.pumpWidget(bootstrap(true));
    await tester.pump();
    expect(registrations, 1);
    await tester.pumpWidget(bootstrap(false));
    await tester.pumpWidget(bootstrap(true));
    await tester.pump();
    expect(registrations, 1);
    await tester.pump(const Duration(seconds: 5));
    await tester.pump();
    expect(registrations, 2);
  });

  testWidgets('a phone that cannot get push stops retrying until a toggle', (
    tester,
  ) async {
    var registrations = 0;
    Widget bootstrap({int retrySignal = 0, String key = 'community|relay|0'}) =>
        KuzzPushRegistrationBootstrap(
          shouldRegister: true,
          attemptKey: key,
          retrySignal: retrySignal,
          startRegistration: () async {
            registrations += 1;
            throw const KuzzPushUnavailableOnThisPhone();
          },
          child: const SizedBox(),
        );
    addTearDown(() => pushSetupError.value = null);

    await tester.pumpWidget(bootstrap());
    await tester.pump();
    expect(registrations, 1);
    expect(pushSetupError.value, isNull);
    await tester.pump(const Duration(minutes: 30));
    await tester.pumpWidget(bootstrap(retrySignal: 1));
    await tester.pump();
    expect(registrations, 1);

    // Turning push off and on is a new attempt.
    await tester.pumpWidget(
      bootstrap(retrySignal: 1, key: 'community|relay|2'),
    );
    await tester.pump();
    expect(registrations, 2);
  });

  testWidgets('a later APNs registration failure is retried', (tester) async {
    var registrations = 0;
    apnsDeviceToken.value = null;
    await tester.pumpWidget(
      KuzzPushRegistrationBootstrap(
        shouldRegister: true,
        attemptKey: 'community|relay',
        startRegistration: () async {
          registrations += 1;
        },
        child: const SizedBox(),
      ),
    );
    expect(registrations, 1);

    apnsRegistrationFailures.value += 1;
    await tester.pump(const Duration(seconds: 5));
    await tester.pump();
    expect(registrations, 2);

    // The same failure again still counts, now after 10 s.
    apnsRegistrationFailures.value += 1;
    await tester.pump(const Duration(seconds: 9));
    expect(registrations, 2);
    await tester.pump(const Duration(seconds: 1));
    await tester.pump();
    expect(registrations, 3);
  });

  test('completed bootstrap attempt can run again for later work', () {
    final gate = KuzzPushAttemptGate();
    addTearDown(gate.dispose);

    expect(gate.tryBegin('attempt'), isTrue);
    gate.complete('attempt');
    expect(gate.tryBegin('attempt'), isTrue);
  });

  test('an equal descriptor is the same publication attempt', () {
    final subscription = KuzzPushSubscription(
      filter: KuzzPushFilter(kinds: const [9], pTags: [_hex('a')]),
      notificationClass: 'default',
    );
    String key(KuzzPushLeaseDescriptor descriptor, {int userRetry = 0}) =>
        kuzzPushPublicationAttemptKey(
          communityId: 'community',
          relayBaseUrl: 'https://relay.example',
          token: 'token',
          descriptor: descriptor,
          subscriptions: [subscription],
          userRetry: userRetry,
        );
    final first = _descriptor(keyId: 'relay-v1', pubkey: _hex('b'));
    final refetched = _descriptor(keyId: 'relay-v1', pubkey: _hex('b'));

    expect(identical(first, refetched), isFalse);
    expect(first, refetched);
    expect(first.hashCode, refetched.hashCode);
    expect(key(refetched), key(first));
    expect(key(first, userRetry: 1), isNot(key(first)));
  });

  test('publication attempt changes when the relay executor rotates', () {
    final subscription = KuzzPushSubscription(
      filter: KuzzPushFilter(kinds: const [9], pTags: [_hex('a')]),
      notificationClass: 'default',
    );
    final original = kuzzPushPublicationAttemptKey(
      communityId: 'community',
      relayBaseUrl: 'https://relay.example',
      token: 'token',
      descriptor: _descriptor(keyId: 'relay-v1', pubkey: _hex('b')),
      subscriptions: [subscription],
    );

    expect(
      kuzzPushPublicationAttemptKey(
        communityId: 'community',
        relayBaseUrl: 'https://relay.example',
        token: 'token',
        descriptor: _descriptor(keyId: 'relay-v2', pubkey: _hex('b')),
        subscriptions: [subscription],
      ),
      isNot(original),
    );
    expect(
      kuzzPushPublicationAttemptKey(
        communityId: 'community',
        relayBaseUrl: 'https://relay.example',
        token: 'token',
        descriptor: _descriptor(keyId: 'relay-v1', pubkey: _hex('c')),
        subscriptions: [subscription],
      ),
      isNot(original),
    );
  });

  test('relay capability alone does not activate push without opt-in', () {
    final disabled = Community.create(
      name: 'Team',
      relayUrl: 'wss://relay.example',
    );
    final enabled = disabled.copyWith(pushNotificationsEnabled: true);

    expect(
      kuzzPushLifecycleEnabled(
        community: disabled,
        descriptor: _descriptor(keyId: 'relay-v1', pubkey: _hex('b')),
      ),
      isFalse,
    );
    expect(
      kuzzPushLifecycleEnabled(
        community: enabled,
        descriptor: _descriptor(keyId: 'relay-v1', pubkey: _hex('b')),
      ),
      isTrue,
    );
    expect(
      kuzzPushLifecycleEnabled(community: enabled, descriptor: null),
      isFalse,
    );
  });

  test('pending opt-out tombstone keeps active push lifecycle disabled', () {
    final subscription = KuzzPushSubscription(
      filter: KuzzPushFilter(kinds: const [9], pTags: [_hex('a')]),
      notificationClass: 'default',
    );
    final community =
        Community.create(
          name: 'Team',
          relayUrl: 'wss://relay.example',
        ).copyWith(
          pushNotificationsEnabled: false,
          pushSubscriptionState:
              KuzzPushLeaseSubscriptionState.desired(desired: [subscription])
                  .withAccepted(subscriptions: [subscription], generation: 3)
                  .withPendingTombstone(4),
        );

    expect(
      kuzzPushLifecycleEnabled(
        community: community,
        descriptor: _descriptor(keyId: 'relay-v1', pubkey: _hex('b')),
      ),
      isFalse,
    );
  });

  test(
    'relay commit followed by local failure retries at a newer generation',
    () async {
      var durableCursor = 0;
      var relayGeneration = 0;
      var acceptedGeneration = 0;
      var failLocalSave = true;

      Future<int> reserve() async => ++durableCursor;
      Future<void> publish(int generation) async {
        expect(generation, greaterThan(relayGeneration));
        relayGeneration = generation;
      }

      Future<bool> markAccepted(int generation) async {
        if (failLocalSave) {
          failLocalSave = false;
          throw StateError('injected local persistence failure');
        }
        acceptedGeneration = generation;
        return true;
      }

      await expectLater(
        publishKuzzPushLeaseRecoverably(
          reserveGeneration: reserve,
          publish: publish,
          markAccepted: markAccepted,
        ),
        throwsStateError,
      );
      expect(relayGeneration, 1);
      expect(acceptedGeneration, 0);

      await publishKuzzPushLeaseRecoverably(
        reserveGeneration: reserve,
        publish: publish,
        markAccepted: markAccepted,
      );
      expect(relayGeneration, 2);
      expect(acceptedGeneration, 2);
    },
  );

  group('KuzzPushBootstrap', () {
    setUp(() => apnsDeviceToken.value = 'device-token');
    tearDown(() {
      apnsDeviceToken.value = null;
      pushSetupError.value = null;
    });

    testWidgets(
      'a relay that keeps rejecting the lease backs off instead of looping',
      (tester) async {
        final community = _pushCommunity('Team');
        var publishes = 0;
        await _pumpBootstrap(
          tester,
          community: community,
          publishLease: (ref, config, community, memberPubkey, relay) async {
            await Future<void>.value();
            publishes += 1;
            // Reserving the generation writes the account, and the server's
            // push support is fetched again; then the relay says no.
            ref.read(_activeCommunity.notifier).reserve(publishes);
            ref.invalidate(currentRelayPushDescriptorProvider);
            throw StateError('relay rejected the lease');
          },
        );
        expect(publishes, 1);
        expect(pushSetupError.value?.communityId, community.id);

        Future<void> wait(int seconds) async {
          await tester.pump(Duration(seconds: seconds));
          await tester.pump();
        }

        await wait(4);
        expect(publishes, 1);
        await wait(1);
        expect(publishes, 2);
        await wait(9);
        expect(publishes, 2);
        await wait(1);
        expect(publishes, 3);
        await wait(19);
        expect(publishes, 3);
        await wait(1);
        expect(publishes, 4);

        // A reconnect is an explicit retry: it runs at once, and the next
        // wait keeps growing (80 s after the 40 s one it replaced).
        final session =
            ProviderScope.containerOf(
                  tester.element(find.byType(SizedBox)),
                ).read(relaySessionProvider.notifier)
                as _Session;
        session.setStatus(SessionStatus.reconnecting);
        await tester.pump();
        expect(publishes, 4);
        session.setStatus(SessionStatus.connected);
        await tester.pump();
        await tester.pump();
        expect(publishes, 5);
        await wait(79);
        expect(publishes, 5);
        await wait(1);
        expect(publishes, 6);
      },
      skip: !Env.pushGatewayConfigured,
    );

    testWidgets('turning push off and on retries at once', (tester) async {
      var publishes = 0;
      await _pumpBootstrap(
        tester,
        community: _pushCommunity('Team'),
        publishLease: (ref, config, community, memberPubkey, relay) async {
          publishes += 1;
          throw StateError('relay rejected the lease');
        },
      );
      expect(publishes, 1);
      kuzzPushUserRetries.value += 1;
      await tester.pump();
      await tester.pump();
      expect(publishes, 2);
    }, skip: !Env.pushGatewayConfigured);

    testWidgets(
      'signing out and back in to the same account publishes again (R2)',
      (tester) async {
        final community = _pushCommunity('Team');
        var publishes = 0;
        var registrations = 0;
        await _pumpBootstrap(
          tester,
          community: community,
          publishLease: (ref, config, community, memberPubkey, relay) async {
            publishes += 1;
            return _grant(
              expiresAt:
                  DateTime.now()
                      .add(const Duration(hours: 1))
                      .millisecondsSinceEpoch ~/
                  1000,
            );
          },
          startRegistration: () async => registrations += 1,
        );
        expect(publishes, 1);
        expect(registrations, 1);
        final container = ProviderScope.containerOf(
          tester.element(find.byType(SizedBox)),
        );
        final session =
            container.read(relaySessionProvider.notifier) as _Session;

        // A plain reconnect keeps the held lease.
        session.setStatus(SessionStatus.reconnecting);
        await tester.pump();
        session.setStatus(SessionStatus.connected);
        await tester.pump();
        await tester.pump();
        expect(publishes, 1);
        expect(registrations, 1);

        // Signed out: the active account reads as push off.
        container
            .read(_activeCommunity.notifier)
            .set(
              community.copyWith(
                signedOut: true,
                pushNotificationsEnabled: false,
              ),
            );
        await tester.pump();
        await tester.pump();
        expect(publishes, 1);

        // Signed back in to the same account in the same run.
        container.read(_activeCommunity.notifier).set(community);
        await tester.pump();
        await tester.pump();
        expect(publishes, 2);
        expect(registrations, 2);

        // Push turned off and on again also publishes, even without the
        // toggle's retry bump.
        container
            .read(_activeCommunity.notifier)
            .set(community.copyWith(pushNotificationsEnabled: false));
        await tester.pump();
        await tester.pump();
        container.read(_activeCommunity.notifier).set(community);
        await tester.pump();
        await tester.pump();
        expect(publishes, 3);
      },
      skip: !Env.pushGatewayConfigured,
    );

    testWidgets('a setup failure does not follow another account', (
      tester,
    ) async {
      final a = _pushCommunity('A');
      final b = _pushCommunity('B');
      await _pumpBootstrap(
        tester,
        community: a,
        publishLease: (ref, config, community, memberPubkey, relay) async =>
            throw StateError('relay rejected the lease'),
      );
      pushSetupError.value = KuzzPushSetupError(
        communityId: a.id,
        message: 'relay rejected the lease',
      );
      final container = ProviderScope.containerOf(
        tester.element(find.byType(SizedBox)),
      );

      // Another account becomes active.
      container.read(_activeCommunity.notifier).set(b);
      await tester.pump();
      await tester.pump();
      expect(pushSetupError.value?.communityId, isNot(a.id));

      // Push turned off for the account the failure belongs to.
      pushSetupError.value = KuzzPushSetupError(
        communityId: b.id,
        message: 'relay rejected the lease',
      );
      container
          .read(_activeCommunity.notifier)
          .set(b.copyWith(pushNotificationsEnabled: false));
      await tester.pump();
      await tester.pump();
      expect(pushSetupError.value, isNull);

      // Signed out: no active account.
      container.read(_activeCommunity.notifier).set(b);
      await tester.pump();
      await tester.pump();
      pushSetupError.value = KuzzPushSetupError(
        communityId: b.id,
        message: 'relay rejected the lease',
      );
      container.read(_activeCommunity.notifier).set(null);
      await tester.pump();
      await tester.pump();
      expect(pushSetupError.value, isNull);
    });
  });
}

class _Session extends RelaySessionNotifier {
  @override
  SessionState build() => const SessionState(status: SessionStatus.connected);

  void setStatus(SessionStatus status) => state = SessionState(status: status);
}

class _NoCommunities extends CommunityListNotifier {
  @override
  Future<List<Community>> build() async => const [];
}

final _activeCommunity = NotifierProvider<_ActiveCommunity, Community?>(
  _ActiveCommunity.new,
);

class _ActiveCommunity extends Notifier<Community?> {
  static Community? initial;

  @override
  Community? build() => initial;

  void set(Community? community) => state = community;

  /// What reserving a lease generation writes: a new record whose push
  /// bookkeeping changed.
  void reserve(int generation) => state = state!.copyWith(
    pushSubscriptionState: state!.pushSubscriptionState.withReservedGeneration(
      generation,
    ),
  );
}

Future<void> _pumpBootstrap(
  WidgetTester tester, {
  required Community? community,
  required KuzzPushLeasePublisher publishLease,
  Future<void> Function()? startRegistration,
}) async {
  _ActiveCommunity.initial = community;
  await tester.pumpWidget(
    ProviderScope(
      overrides: [
        relaySessionProvider.overrideWith(_Session.new),
        communityListProvider.overrideWith(_NoCommunities.new),
        activeCommunityProvider.overrideWith(
          (ref) async => ref.watch(_activeCommunity),
        ),
        // Every fetch is a new instance with the same content.
        kuzzPushDescriptorFetcherProvider.overrideWithValue(
          (_) async => _descriptor(keyId: 'relay-v1', pubkey: _hex('b')),
        ),
        kuzzPushLeaseRevocationOutboxProvider.overrideWithValue(
          KuzzPushLeaseRevocationOutbox(
            storage: KuzzPushLeaseRevocationStorage(
              secure: FakeSecureStorage(),
            ),
            publisher: (_) async {},
          ),
        ),
      ],
      child: KuzzPushBootstrap(
        publishLease: publishLease,
        startRegistration: startRegistration ?? () async {},
        child: const SizedBox(),
      ),
    ),
  );
  // The account, the connection and the server's push support load.
  for (var i = 0; i < 5; i++) {
    await tester.pump();
  }
}

Community _pushCommunity(String name) =>
    Community.create(
      name: name,
      relayUrl: 'wss://relay.example',
      nsec: nostr.Keys.generate().nsec,
    ).copyWith(
      pushNotificationsEnabled: true,
      pushSubscriptionState: KuzzPushLeaseSubscriptionState.desired(
        desired: [
          KuzzPushSubscription(
            filter: KuzzPushFilter(kinds: const [9], pTags: [_hex('a')]),
            notificationClass: 'default',
          ),
        ],
      ),
    );

KuzzPushLeaseDescriptor _descriptor({
  required String keyId,
  required String pubkey,
}) => KuzzPushLeaseDescriptor(
  origin: 'wss://relay.example',
  executorKeyId: keyId,
  executorPubkey: pubkey,
  transport: 'apns',
  maxLeaseTtlSeconds: 3600,
  maxContentLength: 4096,
  maxPlaintextLength: 4096,
  maxEndpointLength: 2048,
  maxStringLength: 512,
);

String _hex(String character) => List.filled(64, character).join();

KuzzPushEndpointGrant _grant({required int expiresAt}) => KuzzPushEndpointGrant(
  relayOrigin: 'wss://relay.example',
  relayPubkey: _hex('b'),
  installationId: '0' * 32,
  endpointGrant: 'grant',
  endpointHash: _hex('f'),
  appProfile: 'kuzz-ios-dogfood',
  endpointEpoch: 1,
  generation: 1,
  expiresAt: expiresAt,
);
