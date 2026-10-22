import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/features/channels/channel_mutes/channel_mutes_manager.dart';
import 'package:kuzz/features/channels/channel_names/channel_names_manager.dart';
import 'package:kuzz/features/channels/channel_names/channel_names_storage.dart';
import 'package:nostr/nostr.dart' as nostr;
import 'package:shared_preferences/shared_preferences.dart';

void main() {
  setUp(() => SharedPreferences.setMockInitialValues({}));

  group('private chat names', () {
    test('names are trimmed, capped, and blank means restore', () {
      expect(normalizeChannelName('  Trip  '), 'Trip');
      expect(normalizeChannelName('   '), isNull);
      expect(normalizeChannelName(null), isNull);
      expect(normalizeChannelName('x' * 100)!.length, channelNameMaxLength);
    });

    test('the synced document is {"<channel_id>":"name"}', () {
      const store = ChannelNameStore(
        channels: {
          'c1': ChannelNameEntry(name: 'Trip', updatedAt: 5),
          'c2': ChannelNameEntry(name: null, updatedAt: 6),
        },
      );
      expect(store.toRemote(), {'c1': 'Trip'});
      expect(parseRemoteChannelNames(jsonEncode(store.toRemote())), {
        'c1': 'Trip',
      });
      expect(parseRemoteChannelNames('{"c1": 3, "c2": " "}'), isEmpty);
      expect(parseRemoteChannelNames('[]'), isNull);
      expect(parseRemoteChannelNames('nope'), isNull);
    });

    test('remote wins unless this device changed the chat later', () {
      const local = ChannelNameStore(
        channels: {
          'old': ChannelNameEntry(name: 'Old', updatedAt: 10),
          'newer': ChannelNameEntry(name: 'Mine', updatedAt: 30),
          'gone': ChannelNameEntry(name: 'Gone', updatedAt: 10),
        },
      );
      final merged = mergeRemoteChannelNames(local, {
        'old': 'Theirs',
        'newer': 'Theirs',
        'added': 'Added',
      }, 20);
      expect(merged.nameFor('old'), 'Theirs');
      expect(merged.nameFor('newer'), 'Mine');
      expect(merged.nameFor('added'), 'Added');
      // Left out of the newer document: restored.
      expect(merged.nameFor('gone'), isNull);
    });

    test('the local store survives a JSON round trip', () async {
      final prefs = await SharedPreferences.getInstance();
      final storage = ChannelNamesStorage(prefs);
      const store = ChannelNameStore(
        channels: {
          'c1': ChannelNameEntry(name: 'Trip', updatedAt: 5),
          'c2': ChannelNameEntry(name: null, updatedAt: 6),
        },
      );
      await storage.write('me', store);
      final read = storage.read('me');
      expect(read.nameFor('c1'), 'Trip');
      expect(read.channels['c2'], store.channels['c2']);
      expect(storage.read('someone-else').channels, isEmpty);
    });

    test('Rename for me and Restore name change only this chat', () async {
      final prefs = await SharedPreferences.getInstance();
      final keys = nostr.Keys.generate();
      var changes = 0;
      final manager = ChannelNamesManager(
        pubkey: keys.public,
        prefs: prefs,
        crypto: ChannelMutesCrypto(keys.nsec, keys.public),
        relaySession: null,
        signedEventRelay: null,
        remoteEnabled: false,
        onChanged: () => changes += 1,
      );
      addTearDown(manager.dispose);

      manager.setName('c1', '  Family ');
      expect(manager.store.nameFor('c1'), 'Family');
      expect(manager.store.nameFor('c2'), isNull);
      expect(changes, 1);

      // Same name again: nothing to do.
      manager.setName('c1', 'Family');
      expect(changes, 1);

      manager.setName('c1', null);
      expect(manager.store.nameFor('c1'), isNull);
      expect(manager.store.toRemote(), isEmpty);
      expect(changes, 2);
      expect(
        ChannelNamesStorage(prefs).read(keys.public).nameFor('c1'),
        isNull,
      );
    });
  });
}
