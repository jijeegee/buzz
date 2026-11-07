import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:kuzz/shared/community/community.dart';
import 'package:kuzz/shared/community/community_provider.dart';
import 'package:kuzz/shared/community/community_storage.dart';
import 'package:kuzz/shared/push/push_bridge.dart';
import 'package:nostr/nostr.dart' as nostr;

import '../community/community_storage_test.dart';

final _hex32 = RegExp(r'^[0-9a-f]{32}$');

Community _community({bool push = true, String? token, bool out = false}) =>
    Community(
      id: 'c1',
      name: 'C',
      relayUrl: 'https://relay.example',
      pubkey: 'a' * 64,
      pushNotificationsEnabled: push,
      pushInboxToken: token,
      signedOut: out,
      addedAt: DateTime.fromMillisecondsSinceEpoch(0),
    );

void main() {
  group('Community.pushInboxToken', () {
    test('survives toJson/fromJson and is never made by toJson', () {
      final token = 'ab' * 16;
      final saved = Community.fromJson(_community(token: token).toJson());
      expect(saved.pushInboxToken, token);
      final none = _community().toJson();
      expect(none.containsKey('pushInboxToken'), isFalse);
      expect(Community.fromJson(none).pushInboxToken, isNull);
    });

    test('a malformed stored token is refused', () {
      final json = _community().toJson()..['pushInboxToken'] = 'AB' * 16;
      expect(() => Community.fromJson(json), throwsFormatException);
    });

    test('made once push is on, kept after, none when off or signed out', () {
      final made = _community().withPushInboxToken();
      expect(made.pushInboxToken, matches(_hex32));
      expect(made.withPushInboxToken().pushInboxToken, made.pushInboxToken);
      expect(
        _community(push: false).withPushInboxToken().pushInboxToken,
        isNull,
      );
      expect(_community(out: true).withPushInboxToken().pushInboxToken, isNull);
    });

    test('resume and a different account replace it; same account keeps', () {
      final token = 'cd' * 16;
      final c = _community(token: token);
      expect(c.resumeSession().pushInboxToken, matches(_hex32));
      expect(c.resumeSession().pushInboxToken, isNot(token));
      expect(c.withCredentials(pubkey: 'a' * 64).pushInboxToken, token);
      expect(c.withCredentials(pubkey: 'A' * 64).pushInboxToken, token);
      final other = c.withCredentials(pubkey: 'b' * 64).pushInboxToken;
      expect(other, matches(_hex32));
      expect(other, isNot(token));
      final keys = nostr.Keys.generate();
      final keyed = c.copyWith(pubkey: null, nsec: keys.nsec);
      expect(keyed.withCredentials(nsec: keys.nsec).pushInboxToken, token);
    });
  });

  group('snapshot export', () {
    test('carries the token only while the inbox flag is on', () {
      final token = 'ef' * 16;
      final c = _community(token: token);
      expect(
        kuzzPushCommunitySnapshotFor(c, inbox: true).toJson(),
        containsPair('inboxToken', token),
      );
      expect(
        kuzzPushCommunitySnapshotFor(c, inbox: false).toJson(),
        isNot(contains('inboxToken')),
      );
      expect(
        kuzzPushCommunitySnapshotFor(
          _community(token: token, out: true),
          inbox: true,
        ).toJson(),
        isNot(contains('inboxToken')),
      );
    });
  });

  group('CommunityListNotifier', () {
    late CommunityStorage storage;
    late List<List<Community>> snapshots;
    late ProviderContainer container;

    setUp(() {
      storage = CommunityStorage(secure: FakeSecureStorage());
      snapshots = [];
    });
    tearDown(() => container.dispose());

    ProviderContainer open() => ProviderContainer(
      overrides: [
        communityStorageProvider.overrideWithValue(storage),
        communitySnapshotWriterProvider.overrideWithValue((items) async {
          snapshots.add(List.of(items));
        }),
      ],
    );

    test('turning push on persists a token that later syncs keep', () async {
      final community = Community.create(
        name: 'C',
        relayUrl: 'https://relay.example',
        pubkey: 'a' * 64,
      );
      await storage.save(community);
      container = open();
      final notifier = container.read(communityListProvider.notifier);
      await container.read(communityListProvider.future);
      expect((await storage.loadAll()).single.pushInboxToken, isNull);

      await notifier.setPushNotificationsEnabled(community.id, true);
      final token = (await storage.loadAll()).single.pushInboxToken;
      expect(token, matches(_hex32));
      expect(snapshots.last.single.pushInboxToken, token);

      await notifier.syncSnapshot();
      container.invalidate(communityListProvider);
      await container.read(communityListProvider.future);
      expect((await storage.loadAll()).single.pushInboxToken, token);
    });

    test('a push-on record saved before tokens gets one at load', () async {
      await storage.save(
        Community.create(
          name: 'C',
          relayUrl: 'https://relay.example',
          pubkey: 'a' * 64,
        ).copyWith(pushNotificationsEnabled: true),
      );
      container = open();
      final loaded = await container.read(communityListProvider.future);
      expect(loaded.single.pushInboxToken, matches(_hex32));
      expect(
        (await storage.loadAll()).single.pushInboxToken,
        loaded.single.pushInboxToken,
      );
    });

    test('a new account on the same server gets a new token', () async {
      final community = Community.create(
        name: 'C',
        relayUrl: 'https://relay.example',
        pubkey: 'a' * 64,
      ).copyWith(pushNotificationsEnabled: true).withPushInboxToken();
      await storage.save(community);
      container = open();
      await container.read(communityListProvider.future);
      await storage.save(community.withCredentials(pubkey: 'b' * 64));
      container.invalidate(communityListProvider);
      await container.read(communityListProvider.future);
      final saved = (await storage.loadAll()).single;
      expect(saved.pushInboxToken, matches(_hex32));
      expect(saved.pushInboxToken, isNot(community.pushInboxToken));
    });
  });
}
