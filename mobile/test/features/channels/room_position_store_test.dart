import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:kuzz/features/channels/channel_management_provider.dart';
import 'package:kuzz/features/channels/room_position_store.dart';
import 'package:kuzz/shared/theme/theme_provider.dart';
import 'package:shared_preferences/shared_preferences.dart';

void main() {
  late SharedPreferences prefs;
  var clock = 0;

  setUp(() async {
    SharedPreferences.setMockInitialValues({});
    prefs = await SharedPreferences.getInstance();
    clock = 0;
  });

  RoomPositionStore store({String pubkey = 'alice'}) => RoomPositionStore(
    prefs,
    relay: 'https://relay.example',
    pubkey: pubkey,
    now: () => ++clock,
  );

  RoomPositionReading at(String id, {int createdAt = 100, double edge = 0.4}) =>
      RoomPositionReading.at(id: id, createdAt: createdAt, edge: edge);

  test('saves and restores a room position under a kuzz key', () {
    store().record('room', at('m1', createdAt: 42, edge: 0.25));

    final restored = store().read('room');
    expect(restored?.id, 'm1');
    expect(restored?.createdAt, 42);
    expect(restored?.edge, 0.25);
    expect(prefs.getKeys(), {
      'kuzz-room-position.v1:https://relay.example:alice',
    });
  });

  test('a later save replaces the room position', () {
    final positions = store();
    positions.record('room', at('m1'));
    positions.record('room', at('m2'));

    expect(store().read('room')?.id, 'm2');
  });

  test('keeps positions apart per account', () {
    store(pubkey: 'alice').record('room', at('m1'));

    expect(store(pubkey: 'bob').read('room'), isNull);
    expect(store(pubkey: 'alice').read('room')?.id, 'm1');
  });

  test('keeps at most the 200 most recently saved rooms', () {
    final positions = store();
    for (var i = 0; i < maxRoomPositions + 5; i++) {
      positions.record('room-$i', at('m$i'));
    }
    // Saving an old room again makes it recent.
    positions.record('room-0', at('again'));
    positions.record('room-new', at('m-new'));

    final saved = (jsonDecode(prefs.getString(store().prefsKey)!) as Map).keys;
    expect(saved, hasLength(maxRoomPositions));
    expect(store().read('room-0')?.id, 'again');
    expect(store().read('room-new'), isNotNull);
    for (var i = 1; i <= 6; i++) {
      expect(store().read('room-$i'), isNull, reason: 'room-$i');
    }
    expect(store().read('room-7'), isNotNull);
  });

  test('a reading at the newest row forgets the room', () {
    final positions = store();
    positions.record('room', at('m1'));
    positions.record('other', at('m2'));

    positions.record('room', const RoomPositionReading.atLatest());

    expect(store().read('room'), isNull);
    expect(store().read('other')?.id, 'm2');
  });

  test('clear forgets a room, as sending or a missing row does', () {
    final positions = store();
    positions.record('room', at('m1'));

    positions.clear('room');
    positions.clear('never-saved');

    expect(store().read('room'), isNull);
  });

  test('unreadable stored positions read as none', () async {
    await prefs.setString(store().prefsKey, '{not json');
    expect(store().read('room'), isNull);

    await prefs.setString(
      store().prefsKey,
      jsonEncode({
        'room': {'id': 3},
      }),
    );
    expect(store().read('room'), isNull);
    store().record('room', at('m1'));
    expect(store().read('room')?.id, 'm1');
  });

  test('the provider scopes the store to the signed-in account', () {
    final container = ProviderContainer(
      overrides: [
        savedPrefsProvider.overrideWithValue(prefs),
        currentPubkeyProvider.overrideWith((ref) => 'carol'),
      ],
    );
    addTearDown(container.dispose);
    expect(
      container.read(roomPositionStoreProvider)?.prefsKey,
      endsWith(':carol'),
    );

    final signedOut = ProviderContainer(
      overrides: [
        savedPrefsProvider.overrideWithValue(prefs),
        currentPubkeyProvider.overrideWith((ref) => null),
      ],
    );
    addTearDown(signedOut.dispose);
    expect(signedOut.read(roomPositionStoreProvider), isNull);
  });
}
