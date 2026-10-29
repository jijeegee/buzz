import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:sqflite_common_ffi/sqflite_ffi.dart';

import 'package:kuzz/shared/message_store/head_coverage.dart';
import 'package:kuzz/shared/message_store/message_store.dart';

const _relay = 'wss://relay.example';

/// test-fixtures/message-store-head-coverage.json, shared with desktop
/// (message_store/head_tests.rs).
void main() {
  sqfliteFfiInit();
  final fixture =
      jsonDecode(
            File(
              '../test-fixtures/message-store-head-coverage.json',
            ).readAsStringSync(),
          )
          as Map<String, dynamic>;
  final account = fixture['account'] as String;
  final room = fixture['room'] as String;
  final root = fixture['root'] as String;
  final roomKinds = (fixture['kinds'] as List).cast<int>();
  final threadKinds = (fixture['thread_kinds'] as List).cast<int>();
  final limit = fixture['limit'] as int;

  late Directory dir;
  final stores = <MessageStore>[];

  setUp(() async {
    dir = await Directory.systemTemp.createTemp('message_store_head');
  });

  tearDown(() async {
    for (final store in stores) {
      await store.forget(_relay, account);
    }
    stores.clear();
    await dir.delete(recursive: true);
  });

  MessageStore open() {
    final store = MessageStore(
      factory: () => databaseFactoryFfi,
      directory: () async => dir.path,
    );
    stores.add(store);
    return store;
  }

  Future<List<Object?>> query(
    MessageStore store,
    String sql, [
    List<Object?> args = const [],
  ]) async {
    final db = await databaseFactoryFfi.openDatabase(
      await store.pathOf(store.lease(_relay, account)!),
    );
    try {
      return (await db.rawQuery(sql, args)).toList();
    } finally {
      await db.close();
    }
  }

  Future<Object?> headOf(MessageStore store, String stream) async {
    final spans = await query(
      store,
      'SELECT lo_at, lo_id, hi_at FROM coverage WHERE room = ? AND stream = ?',
      [room, stream],
    );
    expect(spans.length, lessThanOrEqualTo(1));
    if (spans.isEmpty) return null;
    final span = spans.single! as Map<String, Object?>;
    return [span['lo_at'], span['lo_id'], span['hi_at']];
  }

  StoreHeadSpan span(Map<String, dynamic> head) {
    final cursor = head['next_cursor'] as List?;
    final whole = head['has_more'] != true || cursor == null;
    return StoreHeadSpan(
      room: room,
      root: head['root'] as String?,
      kinds: head['root'] == null ? roomKinds : threadKinds,
      liveAt: head['live_at'] as int,
      oldest: whole ? null : (cursor[0] as int, cursor[1] as String),
      ids: (head['ids'] as List).cast<String>().toSet(),
    );
  }

  for (final raw in fixture['cases'] as List) {
    final testCase = raw as Map<String, dynamic>;
    test(testCase['name'], () async {
      final store = open();
      final lease = store.lease(_relay, account);
      store.noteRelayKey(lease, fixture['relay_key'] as String);
      final marks = <String, StoreCapture>{};
      for (final step in (testCase['steps'] as List).cast<Map>()) {
        if (step['events'] case final List events) {
          store.submit(lease, events.cast<Map<String, dynamic>>());
        } else if (step['coverage'] case final List range) {
          store.extendCoverage(lease, [room], range[0] as int, range[1] as int);
        } else if (step['sent'] case final String name) {
          marks[name] = store.headSent(
            lease,
            (step['rooms'] as List).cast<String>(),
          )!;
        } else if (step['head'] case final Map head) {
          final heads = step['token_changed'] == true
              ? <StoreHeadSpan>[]
              : [span(head.cast<String, dynamic>())];
          expect(store.recordHeads(marks[head['sent']], lease, heads), isTrue);
        } else if (step['forget_except'] case final List keep) {
          expect(
            store.forgetRoomsExcept(
              store.capture(lease),
              lease,
              keep.cast<String>().toSet(),
            ),
            isTrue,
          );
        } else if (step['lost'] == true) {
          store.noteLossForTest(lease!);
        }
      }
      await store.flushed();
      final expected = testCase['expect'] as Map<String, dynamic>;
      final liveAt = testCase['live_at'] as int;
      if (expected['room_certain'] case final bool certain) {
        final head = await store.certainRoomHead(
          lease,
          room: room,
          kinds: roomKinds,
          limit: limit,
          liveAt: liveAt,
        );
        expect(head != null, certain, reason: 'room certain');
      }
      if (expected['thread_certain'] case final bool certain) {
        final head = await store.certainThreadHead(
          lease,
          room: room,
          root: root,
          kinds: threadKinds,
          limit: limit,
          liveAt: liveAt,
        );
        expect(head != null, certain, reason: 'thread certain');
      }
      if (expected['events'] case final List events) {
        final ids = await query(store, 'SELECT id FROM events ORDER BY id');
        expect(
          [for (final row in ids) (row! as Map)['id']],
          events,
          reason: 'events',
        );
      }
      final heads = expected['heads'] as Map<String, dynamic>;
      expect(await headOf(store, headStream), heads['room']);
      expect(await headOf(store, threadHeadStream(root)), heads['thread']);
    });
  }

  test('a head needs the mark of its own request', () async {
    final store = open();
    final lease = store.lease(_relay, account);
    store.submit(lease, [
      for (final (id, at) in [('a1', 5000), ('a2', 6000)])
        {
          'id': id,
          'kind': 9,
          'pubkey': 'p',
          'created_at': at,
          'tags': [
            ['h', room],
          ],
          'content': '',
        },
    ]);
    store.extendCoverage(lease, [room], 10000, 20000);
    final whole = StoreHeadSpan(
      room: room,
      root: null,
      kinds: const [9],
      liveAt: 10000,
      oldest: null,
      ids: const {'a1'},
    );
    // A plain capture is no request mark.
    expect(store.recordHeads(store.capture(lease), lease, [whole]), isTrue);
    // A mark answers once.
    final mark = store.headSent(lease, [room]);
    expect(store.recordHeads(mark, lease, const []), isTrue);
    expect(store.recordHeads(mark, lease, [whole]), isTrue);
    // Another scope is refused outright.
    expect(store.recordHeads(mark, null, [whole]), isFalse);
    await store.flushed();
    final ids = await query(store, 'SELECT id FROM events ORDER BY id');
    expect([for (final row in ids) (row! as Map)['id']], ['a1', 'a2']);
    expect(await headOf(store, headStream), isNull);
  });

  test('an expired mark records nothing', () {
    var now = DateTime(2026);
    final marks = StoreHeadMarks(now: () => now);
    marks.open(1, [room]);
    now = now.add(const Duration(minutes: 2, seconds: 1));
    expect(marks.close(1), isNull);
    marks.open(2, [room]);
    marks.note(room, 'x');
    marks.note('other', 'y');
    final closed = marks.close(2)!;
    expect(closed.rooms, {room});
    expect(closed.fresh, {'x'});
  });
}
