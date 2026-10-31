import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:kuzz/shared/community/community.dart';
import 'package:kuzz/shared/community/community_provider.dart';
import 'package:nostr/nostr.dart' as nostr;
import 'package:sqflite_common_ffi/sqflite_ffi.dart';
import 'package:kuzz/shared/message_store/message_store.dart';
import 'package:kuzz/shared/message_store/message_store_provider.dart';
import 'package:kuzz/shared/profile/profile_snapshot.dart';
import 'package:kuzz/shared/profile/user_cache_provider.dart';
import 'package:kuzz/shared/profile/user_profile.dart';
import 'package:kuzz/shared/relay/relay.dart';

const _relay = 'https://relay.example';
const _me = 'aa00000000000000000000000000000000000000000000000000000000000001';
const _other =
    'bb00000000000000000000000000000000000000000000000000000000000002';
const _them =
    'dd00000000000000000000000000000000000000000000000000000000000004';

/// Other people's profiles saved in the account's store file
/// (docs/message-sync.md#launch-order): drawn at launch, asked for again
/// once, replaced by the relay's answer, and never drawn for another account.
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  sqfliteFfiInit();

  late Directory dir;
  late MessageStore store;

  setUp(() async {
    dir = await Directory.systemTemp.createTemp('user_cache_snapshot');
    store = MessageStore(
      factory: () => databaseFactoryFfi,
      directory: () async => dir.path,
    );
  });

  tearDown(() async {
    await store.forget(_relay, _me);
    await store.forget(_relay, _other);
    await dir.delete(recursive: true);
  });

  ProviderContainer containerFor(
    String pubkey,
    _ProfileSession session, {
    bool withCommunity = false,
  }) {
    final container = ProviderContainer(
      overrides: [
        if (withCommunity)
          activeCommunityProvider.overrideWith(
            (ref) async => Community(
              id: 'community',
              name: 'Synthetic',
              relayUrl: _relay,
              addedAt: DateTime(2026),
            ),
          ),
        relayConfigProvider.overrideWith(() => _FakeRelayConfigNotifier()),
        relaySessionProvider.overrideWith(() => session),
        messageStoreProvider.overrideWithValue(store),
        myPubkeyProvider.overrideWithValue(pubkey),
      ],
    );
    addTearDown(container.dispose);
    return container;
  }

  Future<void> settle() async {
    for (var i = 0; i < 5; i++) {
      await Future<void>.delayed(const Duration(milliseconds: 20));
    }
  }

  test('saved profiles draw at launch for their account only', () async {
    await store.writeSnapshots(store.lease(_relay, _me), {
      '$profileSnapshotPrefix$_them': encodeProfileSnapshot(
        const UserProfile(pubkey: _them, displayName: 'Saved Mina'),
        createdAt: 1,
        eventId: 'old',
      ),
    });

    final mine = containerFor(_me, _ProfileSession());
    mine.read(userCacheProvider);
    final theirs = containerFor(_other, _ProfileSession());
    theirs.read(userCacheProvider);
    await settle();

    expect(mine.read(userCacheProvider)[_them]?.displayName, 'Saved Mina');
    expect(theirs.read(userCacheProvider), isEmpty);
  });

  test("a saved profile is asked for again once and the relay's answer "
      'replaces and saves it', () async {
    await store.writeSnapshots(store.lease(_relay, _me), {
      '$profileSnapshotPrefix$_them': encodeProfileSnapshot(
        const UserProfile(pubkey: _them, displayName: 'Saved Mina'),
        createdAt: 1,
        eventId: 'old',
      ),
    });
    final session = _ProfileSession(
      answer: [_profile(_them, 'Mina', createdAt: 5)],
    );
    final container = containerFor(_me, session);
    final cache = container.read(userCacheProvider.notifier);
    await settle();

    // Drawn at once from the saved profile, then asked for.
    expect(cache.get(_them)?.displayName, 'Saved Mina');
    await settle();
    expect(session.requests, 1);
    expect(container.read(userCacheProvider)[_them]?.displayName, 'Mina');

    // Asked for once only.
    cache.get(_them);
    await settle();
    expect(session.requests, 1);

    // The relay's answer is saved for the next launch.
    await store.flushed();
    final saved = await store.readSnapshots(
      store.lease(_relay, _me),
      profileSnapshotPrefix,
      limit: 10,
    );
    expect(
      decodeProfileSnapshot(
        saved['$profileSnapshotPrefix$_them'],
      )?.profile.displayName,
      'Mina',
    );
  });

  Future<void> saveProfiles(Map<String, String> values) =>
      store.writeSnapshots(store.lease(_relay, _me), values);

  // A launch some days ago (this device's clock).
  final lastLaunch = DateTime.now().millisecondsSinceEpoch ~/ 1000 - 5 * 86400;
  const day = profileSinceMarginSeconds;

  String saved(
    String pubkey,
    String name, {
    int createdAt = 0,
    String? id,
    int? verifiedAt,
  }) => encodeProfileSnapshot(
    UserProfile(pubkey: pubkey, displayName: name),
    createdAt: createdAt,
    eventId: id ?? (createdAt == 0 ? '' : 'saved-$createdAt'),
    verifiedAt: verifiedAt ?? (createdAt == 0 ? 0 : lastLaunch),
  );

  Future<SavedProfile?> savedNow(String pubkey) async {
    await store.flushed();
    final all = await store.readSnapshots(
      store.lease(_relay, _me),
      profileSnapshotPrefix,
      limit: 10,
    );
    return decodeProfileSnapshot(all['$profileSnapshotPrefix$pubkey']);
  }

  test(
    'an unchanged saved profile is asked for only if changed since its '
    'last confirmation, stays, is not missing, and is confirmed again',
    () async {
      await saveProfiles({
        '$profileSnapshotPrefix$_them': saved(
          _them,
          'Saved Mina',
          createdAt: 5,
        ),
      });
      // The relay still holds the same event: since excludes it.
      final session = _ProfileSession(
        answer: [_profile(_them, 'Saved Mina', createdAt: 5)],
      );
      final container = containerFor(_me, session);
      final cache = container.read(userCacheProvider.notifier);
      await settle();
      final before = DateTime.now().millisecondsSinceEpoch ~/ 1000;

      expect(cache.get(_them)?.displayName, 'Saved Mina');
      await settle();
      expect(session.requests, 1);
      final filter = session.sent.single.single;
      expect(filter.authors, [_them]);
      expect(filter.since, lastLaunch - day);
      // Kept, revalidated for this run, and never treated as missing.
      expect(
        container.read(userCacheProvider)[_them]?.displayName,
        'Saved Mina',
      );
      expect(await cache.preload([_them]), isTrue);
      cache.get(_them);
      await settle();
      expect(session.requests, 1);
      expect(
        container.read(userCacheProvider)[_them]?.displayName,
        'Saved Mina',
      );
      // Its confirmation moves to now and is saved, the event kept.
      final after = await savedNow(_them);
      expect(after?.verifiedAt, greaterThanOrEqualTo(before));
      expect(after?.createdAt, 5);
      expect(after?.eventId, 'saved-5');
      expect(after?.profile.displayName, 'Saved Mina');
    },
  );

  test('an unchanged profile is exported to the push cache again from its '
      'saved event', () async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    final exported = <Map<Object?, Object?>>[];
    const bridge = MethodChannel('kuzz/push');
    final messenger =
        TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
    messenger.setMockMethodCallHandler(bridge, (call) async {
      final arguments = call.arguments;
      if (arguments is Map && arguments['section'] == 'profiles') {
        exported.add(arguments);
      }
      return null;
    });
    addTearDown(() {
      messenger.setMockMethodCallHandler(bridge, null);
      debugDefaultTargetPlatformOverride = null;
    });
    final signed = NostrEvent.fromJson(
      nostr.Event.from(
        kind: 0,
        createdAt: 7,
        content: '{"name":"Signed Mina"}',
        tags: [],
        secretKey: '1'.padLeft(64, '0'),
      ).toMap(),
    );
    final pubkey = signed.pubkey.toLowerCase();
    await saveProfiles({
      '$profileSnapshotPrefix$pubkey': encodeProfileSnapshot(
        UserProfile(pubkey: pubkey, displayName: 'Signed Mina'),
        createdAt: signed.createdAt,
        eventId: signed.id,
        verifiedAt: lastLaunch,
        event: signed,
      ),
    });
    // The relay has nothing newer: the answer is empty.
    final session = _ProfileSession();
    final container = containerFor(_me, session, withCommunity: true);
    await container.read(activeCommunityProvider.future);
    final cache = container.read(userCacheProvider.notifier);
    await settle();

    expect(await cache.preload([pubkey]), isTrue);
    await settle();
    expect(session.sent.single.single.since, lastLaunch - day);
    expect(
      container.read(userCacheProvider)[pubkey]?.displayName,
      'Signed Mina',
    );
    expect(exported, hasLength(1));
    final events = exported.single['events']! as List;
    expect([for (final e in events) (e as Map)['id']], [signed.id]);
    expect(exported.single['communityId'], 'community');
    // The saved event survives the next save.
    expect((await savedNow(pubkey))?.event?.id, signed.id);
  });

  test('saved profiles are grouped by their last confirmation', () async {
    await saveProfiles({
      '$profileSnapshotPrefix$_them': saved(
        _them,
        'Saved Mina',
        createdAt: 5,
        verifiedAt: lastLaunch,
      ),
      '$profileSnapshotPrefix$_other': saved(
        _other,
        'Saved Jun',
        createdAt: 900,
        verifiedAt: lastLaunch + 2,
      ),
    });
    final session = _ProfileSession();
    final container = containerFor(_me, session);
    final cache = container.read(userCacheProvider.notifier);
    await settle();

    expect(await cache.preload([_them, _other]), isTrue);
    final filters = session.sent.single;
    expect(filters, hasLength(2));
    expect(filters[0].authors, [_them]);
    expect(filters[0].since, lastLaunch - day);
    expect(filters[1].authors, [_other]);
    expect(filters[1].since, lastLaunch + 2 - day);
  });

  NostrEvent tie(String pubkey, String id, String name, int createdAt) =>
      NostrEvent(
        id: id,
        pubkey: pubkey,
        createdAt: createdAt,
        kind: 0,
        tags: const [],
        content: jsonEncode({'name': name}),
        sig: 'sig',
      );

  test(
    'answers the relay would not keep over the saved event are dropped',
    () async {
      await saveProfiles({
        '$profileSnapshotPrefix$_them': saved(
          _them,
          'Saved Mina',
          createdAt: 5,
          id: 'mmm',
        ),
        '$profileSnapshotPrefix$_other': saved(
          _other,
          'Saved Jun',
          createdAt: 9,
        ),
      });
      // The margin can return unchanged or older events: a same-second event
      // with a higher id and an older one both lose to the saved ones.
      final session = _ProfileSession(
        honorSince: false,
        answer: [
          tie(_them, 'zzz', 'Tie Mina', 5),
          _profile(_other, 'Older Jun', createdAt: 3),
        ],
      );
      final container = containerFor(_me, session);
      final cache = container.read(userCacheProvider.notifier);
      await settle();

      expect(await cache.preload([_them, _other]), isTrue);
      await settle();
      expect(session.requests, 1);
      expect(
        container.read(userCacheProvider)[_them]?.displayName,
        'Saved Mina',
      );
      expect(
        container.read(userCacheProvider)[_other]?.displayName,
        'Saved Jun',
      );
    },
  );

  test('a same-second edit with the lower id replaces the saved one, as the '
      'relay keeps it (NIP-01)', () async {
    await saveProfiles({
      '$profileSnapshotPrefix$_them': saved(
        _them,
        'Saved Mina',
        createdAt: lastLaunch,
        id: 'mmm',
      ),
    });
    final session = _ProfileSession(
      answer: [tie(_them, 'aaa', 'Tie Mina', lastLaunch)],
    );
    final container = containerFor(_me, session);
    final cache = container.read(userCacheProvider.notifier);
    await settle();

    expect(await cache.preload([_them]), isTrue);
    expect(container.read(userCacheProvider)[_them]?.displayName, 'Tie Mina');
    expect((await savedNow(_them))?.eventId, 'aaa');
  });

  test('a legacy saved profile answered with its same event keeps that event '
      'for later re-export', () async {
    // Saved before confirmations and events were kept.
    await saveProfiles({
      '$profileSnapshotPrefix$_them': saved(
        _them,
        'Saved Mina',
        createdAt: 4,
        id: 'profile-4',
        verifiedAt: 0,
      ),
    });
    final session = _ProfileSession(
      answer: [_profile(_them, 'Saved Mina', createdAt: 4)],
    );
    final container = containerFor(_me, session);
    final cache = container.read(userCacheProvider.notifier);
    await settle();

    expect(await cache.preload([_them]), isTrue);
    expect(session.sent.single.single.since, isNull);
    final after = await savedNow(_them);
    expect(after?.event?.id, 'profile-4');
    expect(after?.eventId, 'profile-4');
    expect(after?.verifiedAt, greaterThan(0));
  });

  test('a signed event over the size bound is not saved with the profile', () {
    NostrEvent event(int aboutBytes) => NostrEvent(
      id: 'e$aboutBytes',
      pubkey: _them,
      createdAt: 1,
      kind: 0,
      tags: const [],
      content: jsonEncode({'name': 'Mina', 'about': 'x' * aboutBytes}),
      sig: 'sig',
    );
    SavedProfile? roundTrip(NostrEvent e) => decodeProfileSnapshot(
      encodeProfileSnapshot(
        const UserProfile(pubkey: _them, displayName: 'Mina'),
        createdAt: 1,
        eventId: e.id,
        verifiedAt: 2,
        event: e,
      ),
    );

    final small = roundTrip(event(1000));
    expect(small?.event?.id, 'e1000');
    final big = roundTrip(event(profileSnapshotEventMaxBytes));
    expect(big?.event, isNull);
    // The profile itself is still saved.
    expect(big?.profile.displayName, 'Mina');
    expect(big?.eventId, 'e$profileSnapshotEventMaxBytes');
  });

  test('a newer answer replaces the saved profile', () async {
    await saveProfiles({
      '$profileSnapshotPrefix$_them': saved(_them, 'Saved Mina', createdAt: 5),
    });
    final session = _ProfileSession(
      answer: [_profile(_them, 'New Mina', createdAt: lastLaunch + 60)],
    );
    final container = containerFor(_me, session);
    final cache = container.read(userCacheProvider.notifier);
    await settle();

    expect(await cache.preload([_them]), isTrue);
    expect(session.sent.single.single.since, lastLaunch - day);
    expect(container.read(userCacheProvider)[_them]?.displayName, 'New Mina');
    final after = await savedNow(_them);
    expect(after?.createdAt, lastLaunch + 60);
    expect(after?.verifiedAt, greaterThan(lastLaunch));
  });

  test('legacy saved profiles without a confirmation or event are fetched in '
      'full', () async {
    await saveProfiles({
      // Event known, never confirmed (saved before confirmations).
      '$profileSnapshotPrefix$_them': saved(
        _them,
        'Legacy Mina',
        createdAt: 4,
        verifiedAt: 0,
      ),
      // No event at all.
      '$profileSnapshotPrefix$_other': saved(_other, 'Legacy Jun'),
    });
    final session = _ProfileSession(
      answer: [
        _profile(_them, 'Mina', createdAt: 6),
        _profile(_other, 'Jun', createdAt: 2),
      ],
    );
    final container = containerFor(_me, session);
    final cache = container.read(userCacheProvider.notifier);
    await settle();
    final before = DateTime.now().millisecondsSinceEpoch ~/ 1000;

    expect(await cache.preload([_them, _other]), isTrue);
    final filter = session.sent.single.single;
    expect(filter.authors?.toSet(), {_them, _other});
    expect(filter.since, isNull);
    expect(container.read(userCacheProvider)[_them]?.displayName, 'Mina');
    expect(container.read(userCacheProvider)[_other]?.displayName, 'Jun');
    expect((await savedNow(_other))?.verifiedAt, greaterThanOrEqualTo(before));
  });
}

NostrEvent _profile(String pubkey, String name, {required int createdAt}) =>
    NostrEvent(
      id: 'profile-$createdAt',
      pubkey: pubkey,
      createdAt: createdAt,
      kind: 0,
      tags: const [],
      content: jsonEncode({'name': name}),
      sig: 'sig',
    );

class _ProfileSession extends RelaySessionNotifier {
  _ProfileSession({this.answer = const [], this.honorSince = true});

  final List<NostrEvent> answer;

  /// Whether to apply `since` like the relay; off to model a group bound
  /// that returns unchanged or older profiles anyway.
  final bool honorSince;
  int requests = 0;
  final sent = <List<NostrFilter>>[];

  @override
  SessionState build() => const SessionState(status: SessionStatus.connected);

  // A lone filter goes out as a history REQ.
  @override
  Future<List<NostrEvent>> fetchHistory(
    NostrFilter filter, {
    Duration timeout = const Duration(seconds: 8),
  }) => queryRelay([filter]);

  // Several filters go out together in one /query request.
  @override
  Future<List<NostrEvent>> queryRelay(
    List<NostrFilter> filters, {
    Duration timeout = const Duration(seconds: 8),
  }) async {
    if (!filters.every((filter) => filter.kinds.contains(0))) return const [];
    requests++;
    sent.add(filters);
    return [
      for (final event in answer)
        if (filters.any(
          (filter) =>
              (filter.authors ?? const []).contains(event.pubkey) &&
              (!honorSince || event.createdAt >= (filter.since ?? 0)),
        ))
          event,
    ];
  }
}

class _FakeRelayConfigNotifier extends RelayConfigNotifier {
  @override
  RelayConfig build() => const RelayConfig(baseUrl: _relay, nsec: null);
}
