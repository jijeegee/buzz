import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:sqflite_common_ffi/sqflite_ffi.dart';

import 'package:kuzz/features/channels/channel_window.dart';
import 'package:kuzz/shared/message_store/message_store.dart';
import 'package:kuzz/shared/message_store/room_head.dart';
import 'package:kuzz/shared/relay/nostr_models.dart';
import 'package:kuzz/shared/relay/room_bundle_coverage.dart';

const _account =
    'aa00000000000000000000000000000000000000000000000000000000000001';
const _relay = 'wss://relay.example';

List<String> _ids(List<Map<String, dynamic>> events) => [
  for (final event in events) event['id'] as String,
];

List<String> _strings(Object? value) => (value as List).cast<String>();

void main() {
  sqfliteFfiInit();

  test('the room head follows the shared fixture', () async {
    final fixture =
        jsonDecode(
              File(
                '../test-fixtures/message-store-room-head.json',
              ).readAsStringSync(),
            )
            as Map<String, dynamic>;
    final room = fixture['room'] as String;
    final kinds = (fixture['kinds'] as List).cast<int>();
    for (final raw in fixture['cases'] as List) {
      final testCase = raw as Map<String, dynamic>;
      final name = testCase['name'] as String;
      final dir = await Directory.systemTemp.createTemp('message_store_head');
      final store = MessageStore(
        factory: () => databaseFactoryFfi,
        directory: () async => dir.path,
      );
      final lease = store.lease(_relay, _account)!;
      try {
        store.submit(lease, testCase['events'] as List);
        for (final span in testCase['coverage'] as List) {
          store.extendCoverage(lease, [room], span[0] as int, span[1] as int);
        }
        await store.flushed();
        final limit = testCase['limit'] as int;
        final liveAt = testCase['live_at'] as int?;

        // A second connection: closing it must not close the store's.
        final db = await databaseFactoryFfi.openDatabase(
          await store.pathOf(lease),
          options: OpenDatabaseOptions(singleInstance: false),
        );
        final StoredRoomHead head;
        try {
          head = await readRoomHead(
            db,
            room: room,
            kinds: kinds,
            limit: limit,
            liveAt: liveAt,
          );
        } finally {
          await db.close();
        }
        final expected = testCase['expect'] as Map<String, dynamic>;
        expect(head.certain, expected['certain'], reason: name);
        expect(_ids(head.rows), _strings(expected['rows']), reason: name);
        expect(
          _ids(head.aux)..sort(),
          _strings(expected['aux']).toList()..sort(),
          reason: '$name: aux',
        );
        expect(
          [
            for (final summary in head.summaries)
              ((summary['tags'] as List)[0] as List)[1] as String,
          ]..sort(),
          _strings(expected['summaries']).toList()..sort(),
          reason: '$name: summaries',
        );
        final cursor = expected['next_cursor'] as List?;
        expect(
          head.nextCursor,
          cursor == null ? null : (cursor[0] as int, cursor[1] as String),
          reason: '$name: next_cursor',
        );

        final certain = await store.certainRoomHead(
          lease,
          room: room,
          kinds: kinds,
          limit: limit,
          liveAt: liveAt ?? 0,
        );
        if (liveAt != null) {
          expect(certain != null, head.certain, reason: '$name: public read');
        }
        if (certain != null) {
          // The existing window parser takes it as a relay answer.
          final page = parseChannelWindowResponse(
            certain.map(NostrEvent.fromJson).toList(),
            room,
            null,
          );
          expect(
            [for (final row in page.rows) row.event.id],
            _strings(expected['rows']),
            reason: '$name: parsed',
          );
          final bounds = certain.last;
          expect(bounds['kind'], 39006, reason: name);
          final content = jsonDecode(bounds['content'] as String) as Map;
          expect(content['has_more'], isTrue, reason: name);
          expect(
            certain.length,
            1 + limit + head.aux.length + head.summaries.length,
          );
        }
      } finally {
        await store.forget(_relay, _account);
        await dir.delete(recursive: true);
      }
    }
  });

  test(
    'a room is live only from EOSE on the current connection until CLOSED',
    () async {
      final dir = await Directory.systemTemp.createTemp('message_store_live');
      final store = MessageStore(
        factory: () => databaseFactoryFfi,
        directory: () async => dir.path,
      );
      final lease = store.lease(_relay, _account)!;
      var now = 5000;
      final coverage = RoomBundleCoverage(
        scope: () => (store: store, lease: lease),
        query: (_) async => const [],
        nowSecs: () => now,
      );
      final filter = NostrFilter(
        kinds: EventKind.channelEventKinds,
        tags: {
          '#h': ['r1'],
        },
        limit: 0,
      );
      final subscription = Object();
      try {
        expect(coverage.liveAtOf(subscription, 1), isNull);
        coverage.noteEose(subscription, filter, 1);
        expect(coverage.liveAtOf(subscription, 1), 5000);
        expect(coverage.liveAtOf(subscription, 2), isNull);
        now = 5100;
        coverage.noteClosed(subscription);
        expect(coverage.liveAtOf(subscription, 1), isNull);
        coverage.noteEose(subscription, filter, 1);
        expect(coverage.liveAtOf(subscription, 1), 5100);
        await store.flushed();
      } finally {
        await store.forget(_relay, _account);
        await dir.delete(recursive: true);
      }
    },
  );
}
