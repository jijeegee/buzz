import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:kuzz/shared/auth/default_community.dart';
import 'package:kuzz/shared/community/community.dart';
import 'package:kuzz/shared/community/community_provider.dart';
import 'package:kuzz/shared/community/community_storage.dart';
import 'package:kuzz/shared/push/dev_push_lease.dart';
import 'package:kuzz/shared/push/push_bridge.dart';
import 'package:kuzz/shared/push/push_lease_revocation_outbox.dart';
import 'package:kuzz/shared/push/push_snapshot.dart';
import 'package:kuzz/shared/push/push_subscription.dart';
import 'package:nostr/nostr.dart' as nostr;

import '../community/community_storage_test.dart';

/// Notifications show for the active account only: switching away revokes
/// the left account's lease, and switching back publishes it again above
/// every tombstone the relay may have taken.
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  final aKeys = nostr.Keys.generate();
  final bKeys = nostr.Keys.generate();
  final subscription = KuzzPushSubscription(
    filter: KuzzPushFilter(kinds: const [9], pTags: ['a' * 64]),
    notificationClass: 'default',
  );

  late CommunityStorage communityStorage;
  late KuzzPushLeaseRevocationStorage revocations;
  late _Relay relay;
  late _Clock clock;
  late int triggers;
  late KuzzPushLeaseRevocationPublisher publish;
  late KuzzPushLeaseRevocationOutbox outbox;
  late ProviderContainer container;
  late Community a;
  late Community b;

  Community account(String name, nostr.Keys keys, {bool push = true}) =>
      Community.create(
        name: name,
        relayUrl: 'wss://relay.example',
        nsec: keys.nsec,
      ).copyWith(
        pubkey: keys.public,
        pushNotificationsEnabled: push,
        pushSubscriptionState: KuzzPushLeaseSubscriptionState.desired(
          desired: [subscription],
        ).withAccepted(subscriptions: [subscription], generation: 4),
      );

  setUp(() async {
    communityStorage = CommunityStorage(secure: FakeSecureStorage());
    revocations = KuzzPushLeaseRevocationStorage(secure: FakeSecureStorage());
    relay = _Relay();
    clock = _Clock(1_000_000);
    triggers = 0;
    publish = (record) async => relay.tombstone(record);
    outbox = KuzzPushLeaseRevocationOutbox(
      storage: revocations,
      publisher: (record) => publish(record),
      now: clock.call,
      jitter: () => 0,
      reportError: (_, _) {},
      scheduleWake: (_, _) => () {},
    );
  });

  tearDown(() {
    container.dispose();
    debugDefaultTargetPlatformOverride = null;
  });

  Future<KuzzPushLeaseRevocationOutbox> start({
    bool aPush = true,
    bool realSnapshotWriter = false,
  }) async {
    a = account('A', aKeys, push: aPush);
    b = account('B', bKeys);
    await communityStorage.save(a);
    await communityStorage.save(b);
    await communityStorage.saveActiveId(a.id);
    relay.publish(a, 4);
    relay.publish(b, 4);
    final grant = KuzzPushEndpointGrant(
      relayOrigin: 'wss://relay.example',
      relayPubkey: 'a' * 64,
      installationId: '0' * 32,
      endpointGrant: 'opaque',
      endpointHash: 'b' * 64,
      appProfile: kuzzPushAppProfile,
      endpointEpoch: 1,
      generation: 1,
      expiresAt: clock.seconds + 3600,
    );
    container = ProviderContainer(
      overrides: [
        kuzzPushBuildSupportedProvider.overrideWithValue(false),
        defaultCommunityOriginProvider.overrideWithValue(null),
        communityStorageProvider.overrideWithValue(communityStorage),
        if (!realSnapshotWriter)
          communitySnapshotWriterProvider.overrideWithValue((_) async {}),
        communityPushLeaseDeactivatorProvider.overrideWithValue(
          (_, {generation}) async {},
        ),
        communityPushLeaseRevocationEnqueuerProvider.overrideWithValue(
          (community) => outbox.enqueueCommunity(
            community,
            readGrants: () async => [grant],
          ),
        ),
        communityPushLeaseRevocationTriggerProvider.overrideWithValue(
          () async => triggers += 1,
        ),
        communityPushLeaseRevocationWithdrawerProvider.overrideWithValue(
          outbox.withdrawCommunity,
        ),
      ],
    );
    await container.read(communityListProvider.future);
    return outbox;
  }

  CommunityListNotifier notifier() =>
      container.read(communityListProvider.notifier);

  /// What `KuzzPushBootstrap` does for the active account.
  Future<void> publishActive(Community community) async {
    final generation = await notifier().reservePushLeaseGeneration(
      community.id,
    );
    relay.publish(community, generation);
  }

  Future<Community> stored(String id) async =>
      (await communityStorage.loadAll()).singleWhere((c) => c.id == id);

  test('a switch revokes the left account once with its own key', () async {
    await start();

    await notifier().switchCommunity(b.id);

    final record = (await revocations.loadAll()).single;
    expect(record.memberPubkey, aKeys.public);
    expect(record.nsec, aKeys.nsec);
    expect(record.installationId, a.pushLeaseInstallationId);
    expect(record.generation, 6);
    expect(triggers, 1);
    final left = await stored(a.id);
    expect(left.pushNotificationsEnabled, isTrue, reason: 'preference stays');
    expect(
      left.pushSubscriptionState.generationCursor,
      4 + kuzzPushLeaseRevocationBlock,
    );

    await publishActive(b);
    expect(relay.isActive(b), isTrue);
    await outbox.trigger();
    expect(relay.isActive(a), isFalse);
  });

  test('an account that is no longer active cannot publish', () async {
    await start();
    await notifier().switchCommunity(b.id);

    expect(
      () => notifier().reservePushLeaseGeneration(a.id),
      throwsA(isA<StateError>()),
    );
  });

  test('an account with push off queues nothing', () async {
    await start(aPush: false);

    await notifier().switchCommunity(b.id);

    expect(await revocations.loadAll(), isEmpty);
    expect(triggers, 0);
  });

  group('A to B to A ends with A published', () {
    test('after the tombstone landed', () async {
      await start();
      await notifier().switchCommunity(b.id);
      await outbox.trigger();
      expect(relay.isActive(a), isFalse);

      await notifier().switchCommunity(a.id);
      await publishActive(a);

      expect(relay.isActive(a), isTrue);
    });

    test('while the tombstone is still queued', () async {
      await start();
      await notifier().switchCommunity(b.id);

      await notifier().switchCommunity(a.id);
      await publishActive(a);
      final queued = await revocations.loadAll();
      expect(queued.where((r) => r.memberPubkey == aKeys.public), isEmpty);
      await outbox.trigger();

      expect(relay.isActive(a), isTrue);
    });

    test('while the tombstone is in flight', () async {
      await start();
      final gate = Completer<void>();
      final sent = Completer<void>();
      publish = (record) async {
        sent.complete();
        await gate.future;
        relay.tombstone(record);
      };
      await notifier().switchCommunity(b.id);
      final drain = outbox.trigger();
      await sent.future;

      await notifier().switchCommunity(a.id);
      await publishActive(a);
      gate.complete();
      await drain;

      expect(relay.isActive(a), isTrue);
    });

    test('after a retried tombstone', () async {
      await start();
      var attempts = 0;
      publish = (record) async {
        attempts += 1;
        if (attempts == 1) {
          relay.tombstone(record);
          throw StateError('acknowledgement lost');
        }
        relay.tombstone(record);
      };
      await notifier().switchCommunity(b.id);
      await outbox.trigger();
      clock.seconds += 3600 ~/ 2;
      await outbox.trigger();
      expect(attempts, 2);
      expect(relay.generationOf(a), 7);

      await notifier().switchCommunity(a.id);
      await publishActive(a);

      expect(relay.isActive(a), isTrue);
    });
  });

  test('the iOS snapshot names the active account on every switch', () async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    final exported = <String?>[];
    final messenger =
        TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
    const channel = MethodChannel('kuzz/push');
    messenger.setMockMethodCallHandler(channel, (call) async {
      final arguments = call.arguments;
      if (call.method == 'syncPushSnapshot' &&
          arguments is Map &&
          arguments['section'] == 'communities') {
        exported.add(arguments[kuzzPushActiveCommunityKey] as String?);
      }
      return null;
    });
    addTearDown(() => messenger.setMockMethodCallHandler(channel, null));

    await start(realSnapshotWriter: true);
    expect(exported.last, a.id);

    await notifier().switchCommunity(b.id);
    expect(exported.last, b.id);

    await notifier().switchCommunity(a.id);
    expect(exported.last, a.id);
  });

  test('without an active account the snapshot has no active id', () async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    Map<dynamic, dynamic>? exported;
    final messenger =
        TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
    const channel = MethodChannel('kuzz/push');
    messenger.setMockMethodCallHandler(channel, (call) async {
      if (call.method == 'syncPushSnapshot') {
        exported = call.arguments as Map<dynamic, dynamic>;
      }
      return null;
    });
    addTearDown(() => messenger.setMockMethodCallHandler(channel, null));
    container = ProviderContainer();

    await registerKuzzPushCommunitySnapshot([account('A', aKeys)]);

    expect(exported!.containsKey(kuzzPushActiveCommunityKey), isFalse);
  });
}

/// The relay's two gates for one lease address: a newer generation wins, and
/// a later event reactivates an address a tombstone ended.
class _Relay {
  final Map<String, ({int generation, bool active})> _leases = {};

  String _address(String member, String installation) =>
      '$member|$installation';

  void publish(Community community, int generation) => _accept(
    _address(community.pubkey!, community.pushLeaseInstallationId!),
    generation,
    true,
  );

  void tombstone(KuzzPushLeaseRevocationRecord record) => _accept(
    _address(record.memberPubkey, record.installationId),
    record.generation,
    false,
  );

  void _accept(String address, int generation, bool active) {
    final current = _leases[address];
    if (current != null && generation <= current.generation) return;
    _leases[address] = (generation: generation, active: active);
  }

  bool isActive(Community community) =>
      _leases[_address(community.pubkey!, community.pushLeaseInstallationId!)]
          ?.active ??
      false;

  int? generationOf(Community community) =>
      _leases[_address(community.pubkey!, community.pushLeaseInstallationId!)]
          ?.generation;
}

class _Clock {
  _Clock(this.seconds);

  int seconds;

  DateTime call() => DateTime.fromMillisecondsSinceEpoch(seconds * 1000);
}
