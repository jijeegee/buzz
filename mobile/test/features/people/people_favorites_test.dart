import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/features/channels/channel_stars/channel_stars_manager.dart';
import 'package:kuzz/features/people/people_favorites.dart';
import 'package:shared_preferences/shared_preferences.dart';

import '../channels/sidebar_sync_fixture.dart';

const _devA = 'aaaaaaaaaaaaaaaa';
const _devB = 'bbbbbbbbbbbbbbbb';
final _ann = 'a' * 64;
final _bob = 'b' * 64;

Map<String, Object?> _doc(Map<String, List<Object>> registers) => {
  'version': 1,
  'ids': [
    for (final entry in registers.entries)
      if (entry.value[2] == true) entry.key,
  ]..sort(),
  'meta': {'v': 1, 'i': registers},
};

void main() {
  group('registers', () {
    test('higher version wins, then device, then removal over add', () {
      const add = FavoriteRegister(5, _devA, true);
      expect(const FavoriteRegister(6, _devA, false).compareTo(add), 1);
      expect(const FavoriteRegister(5, _devB, false).compareTo(add), 1);
      expect(const FavoriteRegister(5, _devA, false).compareTo(add), 1);
      expect(add.compareTo(add), 0);
    });

    test('merging is last-writer-wins per person and order independent', () {
      final a = {
        _ann: const FavoriteRegister(10, _devA, true),
        _bob: const FavoriteRegister(3, _devA, true),
      };
      final b = {
        _ann: const FavoriteRegister(9, _devB, false),
        _bob: const FavoriteRegister(4, _devB, false),
      };
      final ab = mergeFavoriteTrees(a, b);
      expect(ab, mergeFavoriteTrees(b, a));
      expect(favoriteIdsOf(ab), [_ann]);
      expect(ab[_bob]!.member, isFalse);
    });

    test('the wire document round-trips in desktop shape', () {
      final tree = {
        _bob: const FavoriteRegister(2, _devA, true),
        _ann: const FavoriteRegister(1, _devB, false),
      };
      final doc = encodeFavoritesDoc(tree);
      expect(doc['version'], 1);
      expect(doc['ids'], [_bob]);
      expect((doc['meta']! as Map)['v'], 1);
      expect(decodeFavoritesDoc(doc), tree);
    });

    test('an unknown version is unreadable; bad registers are dropped', () {
      expect(decodeFavoritesDoc({'version': 2, 'ids': []}), isNull);
      expect(
        decodeFavoritesDoc(
          _doc({
            _ann: [1, _devA, true],
            _bob: [1, 'not-a-device', true],
          }),
        ),
        {_ann: const FavoriteRegister(1, _devA, true)},
      );
    });

    test('canonical JSON sorts keys at every depth', () {
      expect(
        canonicalJson({
          'b': 1,
          'a': {'d': true, 'c': null},
        }),
        '{"a":{"c":null,"d":true},"b":1}',
      );
    });
  });

  group('manager', () {
    late SidebarRelay relay;
    late SharedPreferences prefs;
    var now = 1760000000000;

    setUp(() async {
      relay = SidebarRelay();
      prefs = await freshPrefs();
      now = 1760000000000;
    });

    PeopleFavoritesManager manager({bool remote = true}) {
      final created = PeopleFavoritesManager(
        pubkey: relay.pubkey,
        origin: 'https://relay.test',
        prefs: prefs,
        crypto: remote
            ? ChannelStarsCrypto(relay.keys.nsec, relay.pubkey)
            : null,
        relaySession: relay.session,
        signedEventRelay: relay.signer,
        remoteEnabled: remote,
        onChanged: () {},
        debounce: Duration.zero,
        nowMs: () => now,
      );
      addTearDown(created.dispose);
      return created;
    }

    Future<void> settle() async {
      for (var i = 0; i < 5; i++) {
        await Future<void>.delayed(Duration.zero);
        await pumpEventQueue();
      }
    }

    test('adopts the desktop head, then publishes my edit over it', () async {
      relay.stored.add(
        relay.event(
          peopleFavoritesDTag,
          _doc({
            _ann: [now - 5000, _devB, true],
          }),
          1759999990,
        ),
      );
      final favorites = manager();
      await favorites.initialize();
      await settle();
      expect(favorites.ids, {_ann});
      expect(relay.published, isEmpty);

      expect(favorites.add(_bob.toUpperCase()), isTrue);
      await settle();

      final event = relay.published.single;
      expect(event.getTagValue('d'), peopleFavoritesDTag);
      expect(event.getTagValue('t'), peopleFavoritesDTag);
      expect(event.createdAt, greaterThan(1759999990));
      final doc = relay.decrypt(event)! as Map<String, Object?>;
      expect(doc['ids'], [_ann, _bob]);
    });

    test('a removal publishes a tombstone, not an absence', () async {
      final favorites = manager();
      await favorites.initialize();
      favorites.add(_ann);
      await settle();
      now += 1000;
      favorites.remove(_ann);
      await settle();

      final doc = relay.decrypt(relay.published.last)! as Map<String, Object?>;
      expect(doc['ids'], isEmpty);
      final register = ((doc['meta']! as Map)['i']! as Map)[_ann] as List;
      expect(register[2], isFalse);
    });

    test('never writes over a head it cannot read', () async {
      relay.stored.add(
        relay.event(peopleFavoritesDTag, {'version': 2}, 1759999990),
      );
      final favorites = manager();
      await favorites.initialize();
      favorites.add(_ann);
      await settle();

      expect(favorites.ids, {_ann});
      expect(relay.published, isEmpty);
    });

    test('without a signing key the list stays on this device', () async {
      final local = manager(remote: false);
      await local.initialize();
      local.add(_ann);
      await settle();
      expect(relay.published, isEmpty);
      expect(relay.reqs, isEmpty);

      expect(manager(remote: false).ids, {_ann});
    });

    test('refuses a favorite beyond the cap', () async {
      final favorites = manager(remote: false);
      for (var i = 0; i < maxPeopleFavorites; i++) {
        expect(favorites.add(i.toRadixString(16).padLeft(64, '0')), isTrue);
      }
      expect(favorites.add(_ann), isFalse);
      expect(favorites.ids, hasLength(maxPeopleFavorites));
    });
  });
}
