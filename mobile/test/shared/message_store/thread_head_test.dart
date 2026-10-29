import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:sqflite_common_ffi/sqflite_ffi.dart';

import 'package:kuzz/features/channels/thread_window.dart';
import 'package:kuzz/shared/message_store/message_store.dart';
import 'package:kuzz/shared/message_store/thread_head.dart';
import 'package:kuzz/shared/relay/nostr_models.dart';

const _account =
    'aa00000000000000000000000000000000000000000000000000000000000001';
const _relay = 'wss://relay.example';

void main() {
  sqfliteFfiInit();

  test('the thread head follows the shared fixture', () async {
    final fixture =
        jsonDecode(
              File(
                '../test-fixtures/message-store-thread-head.json',
              ).readAsStringSync(),
            )
            as Map<String, dynamic>;
    final room = fixture['room'] as String;
    final root = fixture['root'] as String;
    final kinds = (fixture['kinds'] as List).cast<int>();
    for (final raw in fixture['cases'] as List) {
      final testCase = raw as Map<String, dynamic>;
      final name = testCase['name'] as String;
      final dir = await Directory.systemTemp.createTemp('message_store_thread');
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
        final StoredThreadHead head;
        try {
          head = await readThreadHead(
            db,
            room: room,
            root: root,
            kinds: kinds,
            limit: limit,
            liveAt: liveAt,
          );
        } finally {
          await db.close();
        }
        final expected = testCase['expect'] as Map<String, dynamic>;
        final rows = (expected['rows'] as List).cast<String>();
        expect(head.certain, expected['certain'], reason: name);
        expect(
          [for (final event in head.rows) event['id']],
          rows,
          reason: '$name: rows',
        );
        expect(head.hasMore, expected['has_more'], reason: name);
        final cursor = expected['next_cursor'] as List?;
        expect(
          head.nextCursor,
          cursor == null ? null : (cursor[0] as int, cursor[1] as String),
          reason: '$name: next_cursor',
        );

        if (liveAt == null) continue;
        final response = await store.certainThreadHead(
          lease,
          room: room,
          root: root,
          kinds: kinds,
          limit: limit,
          liveAt: liveAt,
        );
        expect(response != null, head.certain, reason: '$name: public read');
        if (response == null) continue;
        // The existing window parser takes it as a relay answer.
        final page = parseThreadWindowPage(
          response.map(NostrEvent.fromJson).toList(),
          channelId: room,
          rootId: root,
        )!;
        expect(
          [for (final event in page.events) event.id],
          rows,
          reason: '$name: parsed',
        );
        expect(page.hasMore, expected['has_more'], reason: name);
        expect(
          page.nextCursor == null
              ? null
              : [page.nextCursor!.createdAt, page.nextCursor!.eventId],
          cursor,
          reason: '$name: parsed cursor',
        );
      } finally {
        await store.forget(_account);
        await dir.delete(recursive: true);
      }
    }
  });
}
