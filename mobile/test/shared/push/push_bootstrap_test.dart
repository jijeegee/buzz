import 'package:kuzz/shared/push/dev_push_lease.dart';
import 'package:kuzz/shared/community/community.dart';
import 'package:kuzz/shared/push/push_bootstrap.dart';
import 'package:kuzz/shared/push/push_bridge.dart';
import 'package:kuzz/shared/push/push_subscription.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:flutter/widgets.dart';

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
      expect(gate.tryBegin('attempt'), isTrue);
      gate.failed('attempt', retry: () {});
      delays.add(gate.lastRetryDelay!.inSeconds);
    }
    expect(delays, [5, 10, 20, 40, 80, 160, 320, 600, 600, 600]);
  });

  test('success or a different attempt restarts the backoff', () {
    final gate = KuzzPushAttemptGate();
    addTearDown(gate.dispose);
    for (var i = 0; i < 3; i++) {
      gate.tryBegin('attempt');
      gate.failed('attempt', retry: () {});
    }
    expect(gate.lastRetryDelay, const Duration(seconds: 20));

    gate.tryBegin('attempt');
    gate.retryAfter('attempt', delay: const Duration(hours: 1), retry: () {});
    gate.complete('attempt');
    gate.tryBegin('attempt');
    gate.failed('attempt', retry: () {});
    expect(gate.lastRetryDelay, const Duration(seconds: 5));

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
    expect(pushSetupError.value, contains('FCM token unavailable'));

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
}

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
