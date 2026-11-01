import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/features/channels/channels_provider.dart';
import 'package:kuzz/features/channels/room_live_router.dart';
import 'package:kuzz/shared/relay/relay.dart';

/// Shared with desktop's relayRoomBundle tests: both clients must split rooms
/// and place live events identically (docs/message-sync.md).
void main() {
  final fixtures =
      jsonDecode(
            File('../test-fixtures/room-live-routing.json').readAsStringSync(),
          )
          as Map<String, dynamic>;

  test('shared fixture: chunking matches desktop', () {
    for (final fixture in fixtures['chunking'] as List) {
      final generate = fixture['generate'] as Map<String, dynamic>?;
      final rooms = generate == null
          ? [for (final room in fixture['rooms'] as List) room as String]
          : [
              for (var i = 0; i < (generate['count'] as int); i++)
                '${generate['prefix']}$i',
            ];
      // The live set is a set; desktop deduplicates the same way.
      final chunks = chunkChannelIdsForLiveSubscriptions(rooms.toSet());
      final expected = fixture['chunks'] as List?;
      if (expected != null) {
        expect(chunks, expected, reason: fixture['name'] as String);
      }
      final sizes = fixture['chunkSizes'] as List?;
      if (sizes != null) {
        expect(
          chunks.map((chunk) => chunk.length).toList(),
          sizes,
          reason: fixture['name'] as String,
        );
      }
      for (final chunk in chunks) {
        expect(
          chunk.length,
          lessThanOrEqualTo(fixtures['maxRoomsPerSubscription'] as int),
        );
      }
    }
  });

  for (final fixture in fixtures['routing'] as List) {
    test('shared fixture: ${fixture['name']}', () async {
      final chunkRooms = {
        for (final room in fixture['chunkRooms'] as List) room as String,
      };
      final loaded = {
        for (final MapEntry(:key, :value)
            in ((fixture['loaded'] as Map<String, dynamic>?) ?? {}).entries)
          key: {for (final id in value as List) id as String},
      };
      final router = RoomLiveRouter((filter, onEvent) async => () {})
        ..setCoveredRooms(chunkRooms);
      for (final room in {...chunkRooms, ...loaded.keys}) {
        await router.listenRoom(
          room,
          (_) {},
          holdsEvent: (id) => loaded[room]?.contains(id) ?? false,
        );
      }
      for (final before in (fixture['routedBefore'] as List?) ?? const []) {
        router.routeBundleEvent(_event(before as Map), chunkRooms);
      }
      expect(
        router.routeBundleEvent(_event(fixture['event'] as Map), chunkRooms),
        fixture['expectedRoom'],
      );
    });
  }

  for (final fixture in fixtures['delivery'] as List) {
    test('shared fixture: ${fixture['name']}', () async {
      final members = {
        for (final room in fixture['memberRooms'] as List) room as String,
      };
      final quietRooms = {
        for (final room in (fixture['quietRooms'] as List?) ?? const [])
          room as String,
      };
      // A room both member and quiet is a member, as the chunk owner sets it
      // (channels_provider_lifecycle.dart).
      final quietOnly = quietRooms.difference(members);
      final carried = {...members, ...quietRooms};
      final browsed = fixture['browsedRoom'] as String?;
      final own = <void Function(NostrEvent)>[];
      final router =
          RoomLiveRouter((filter, onEvent) async {
              own.add(onEvent);
              return () {};
            })
            ..setQuietRooms(quietOnly)
            ..setBundleRooms(carried)
            ..setCoveredRooms(carried);
      final event = _event(fixture['event'] as Map);
      final room = event.channelId!;
      final inRoom = <String>[];
      final metadata = <String>[];
      final quiet = <String>[];
      router
        ..listenMetadata((e) => metadata.add(e.id))
        ..listenQuiet((e, _) => quiet.add(e.id));
      await router.listenRoom(room, (e) => inRoom.add(e.id));
      final repeat = (fixture['repeat'] as int?) ?? 1;
      for (var i = 0; i < repeat; i++) {
        if (browsed != null) {
          own.single(event);
        } else {
          router.routeBundleEvent(event, carried);
        }
      }
      final name = fixture['name'] as String;
      int once(Object? expected) => expected == true ? 1 : 0;
      expect(inRoom.length, once(fixture['room']), reason: name);
      expect(metadata.length, once(fixture['metadata']), reason: name);
      expect(quiet.length, once(fixture['quiet']), reason: name);
      // The all-room consumers only see member rooms' events
      // (channels_provider_lifecycle.dart); their stores are idempotent.
      expect(
        members.contains(room) && reachesAllRoomListeners(event),
        fixture['allRooms'],
        reason: name,
      );
    });
  }
}

NostrEvent _event(Map json) => NostrEvent(
  id: json['id'] as String,
  pubkey: 'b' * 64,
  createdAt: 100,
  kind: json['kind'] as int,
  tags: [
    for (final tag in json['tags'] as List)
      [for (final value in tag as List) value as String],
  ],
  content: '',
  sig: '',
);
