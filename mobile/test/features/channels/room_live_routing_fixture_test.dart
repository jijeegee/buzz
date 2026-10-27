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
