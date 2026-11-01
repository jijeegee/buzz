import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:sqflite_common_ffi/sqflite_ffi.dart';

import 'package:kuzz/shared/message_store/message_store.dart';

const _relay = 'wss://relay.example';
const _relayKey =
    'cc00000000000000000000000000000000000000000000000000000000000003';

/// Pushed rows (docs/message-sync.md#push-inbox); the `drain` cases of
/// test-fixtures/push-inbox.json.
void main() {
  sqfliteFfiInit();
  final fixture =
      jsonDecode(File('../test-fixtures/push-inbox.json').readAsStringSync())
          as Map<String, dynamic>;
  final account = fixture['drain_account'] as String;

  late Directory dir;
  late MessageStore store;
  late MessageStoreLease lease;

  setUp(() async {
    dir = await Directory.systemTemp.createTemp('message_store_pushed');
    store = MessageStore(
      factory: () => databaseFactoryFfi,
      directory: () async => dir.path,
    );
    lease = store.lease(_relay, account)!;
  });

  tearDown(() async {
    await store.forget(_relay, account);
    await dir.delete(recursive: true);
  });

  int now() => DateTime.now().millisecondsSinceEpoch ~/ 1000;

  Future<T> withDb<T>(Future<T> Function(Database db) body) async {
    // Its own connection: closing it must not close the store's.
    final db = await databaseFactoryFfi.openDatabase(
      await store.pathOf(lease),
      options: OpenDatabaseOptions(singleInstance: false),
    );
    try {
      return await body(db);
    } finally {
      await db.close();
    }
  }

  Future<List<Object?>> column(String sql) => withDb(
    (db) async =>
        (await db.rawQuery(sql)).map((row) => row.values.first).toList(),
  );

  Map<String, dynamic> event(String id, String room, int at) => {
    'id': id,
    'kind': 9,
    'pubkey': 'bb' * 32,
    'created_at': at,
    'tags': [
      ['h', room],
    ],
    'content': id,
  };

  Map<String, dynamic> notice(String id, String room, int at, String body) => {
    'id': id,
    'kind': 40099,
    'pubkey': _relayKey,
    'created_at': at,
    'tags': [
      ['h', room],
    ],
    'content': body,
  };

  Future<void> applyList(Set<String> keep) async {
    expect(store.forgetRoomsExcept(store.capture(lease), lease, keep), isTrue);
    await store.flushed();
  }

  group('drain fixture', () {
    for (final raw in fixture['drain'] as List) {
      final c = raw as Map<String, dynamic>;
      test(c['name'] as String, () async {
        // Opens the file (schema) without loading its rooms, then seeds it.
        await store.writeSnapshots(lease, {'seed': '1'});
        await withDb((db) async {
          if (c['kept'] case final List kept) {
            await db.rawInsert(
              "INSERT INTO sync_state(key, value) VALUES ('kept_rooms', ?)",
              [jsonEncode(kept)],
            );
          }
          for (final room in (c['left'] as List?) ?? const []) {
            await db.rawInsert(
              'INSERT INTO room_purge(room, left_at) VALUES (?, 1)',
              [room],
            );
          }
          for (final MapEntry(:key, :value)
              in ((c['forgot'] as Map?) ?? const {}).entries) {
            await db.rawInsert(
              'INSERT INTO room_forgot(room, at) VALUES (?, ?)',
              [key, value],
            );
          }
        });
        final outcome = await store.submitPushed(
          lease,
          c['events'] as List,
          fetchedAt: c['fetched_at'] as int,
        );
        final written = (c['written'] as List).cast<String>();
        expect(outcome.written, isTrue);
        expect(outcome.accepted, written.toSet());
        expect(await column('SELECT id FROM events ORDER BY id'), written);
        expect(await column('SELECT id FROM pushed_rows ORDER BY id'), written);
      });
    }
  });

  test(
    'a full queue drops the pushed write and keeps the file whole',
    () async {
      await applyList({'r'});
      // Synchronously, so nothing drains in between: the queue fills up.
      for (var i = 0; i < 4096; i++) {
        store.submit(lease, [event('q$i', 'r', 10)]);
      }
      final outcome = store.submitPushed(lease, [
        event('p1', 'r', 20),
      ], fetchedAt: now());
      await store.flushed();
      // A later pass succeeds; the dropped write stays dropped.
      store.submit(lease, [event('late', 'r', 30)]);
      await store.flushed();
      expect(await outcome, PushedWriteOutcome.dropped);
      expect(await column("SELECT id FROM events WHERE id = 'p1'"), isEmpty);
      expect(
        await column("SELECT value FROM sync_state WHERE key = 'lossy'"),
        isEmpty,
        reason: 'pushed rows claim no coverage, so losing them is no loss',
      );
    },
  );

  test('sign-out drops a queued pushed write', () async {
    await applyList({'r'});
    final outcome = store.submitPushed(lease, [
      event('p1', 'r', 20),
    ], fetchedAt: now());
    final forgotten = store.forget(_relay, account);
    expect(await outcome, PushedWriteOutcome.dropped);
    await forgotten;
  });

  test(
    'pushed rows set no born mark: a list fetched meanwhile drops the room',
    () async {
      await applyList({'x', 'y'});
      final capture = store.capture(lease);
      final outcome = await store.submitPushed(lease, [
        event('p1', 'x', 20),
      ], fetchedAt: now());
      expect(outcome.accepted, {'p1'});
      expect(store.forgetRoomsExcept(capture, lease, {'y'}), isTrue);
      await store.flushed();
      expect(await column("SELECT id FROM events WHERE room = 'x'"), isEmpty);
      expect(await column('SELECT id FROM pushed_rows'), isEmpty);
    },
  );

  test('pushed rows add no coverage and leave resume points alone', () async {
    await applyList({'r'});
    store.extendCoverage(lease, ['r'], 100, 200);
    await store.flushed();
    final before = await store.resumePoints(lease, ['r'], 1 << 40);
    final coverage = await column(
      'SELECT room || lo_at || hi_at FROM coverage',
    );
    final outcome = await store.submitPushed(lease, [
      event('p1', 'r', 500),
    ], fetchedAt: now());
    expect(outcome.accepted, {'p1'});
    expect(await store.resumePoints(lease, ['r'], 1 << 40), before);
    expect(
      await column('SELECT room || lo_at || hi_at FROM coverage'),
      coverage,
    );
  });

  test('rooms draw pushed rows only once the relay copy arrives', () async {
    await applyList({'r'});
    store.submit(lease, [event('a1', 'r', 10)]);
    await store.flushed();
    final outcome = await store.submitPushed(lease, [
      event('p1', 'r', 20),
      // Already stored from the relay: not marked, drawn as before.
      event('a1', 'r', 10),
    ], fetchedAt: now());
    expect(outcome.accepted, {'p1', 'a1'});
    expect(await column('SELECT id FROM pushed_rows'), ['p1']);

    Future<List<Object?>> painted() async => [
      for (final message
          in await store.paintableRoomHead(
                lease,
                room: 'r',
                kinds: const [9],
                limit: 50,
              ) ??
              const <Map<String, dynamic>>[])
        if (message['kind'] == 9) message['id'],
    ];
    expect(await painted(), ['a1']);

    store.submit(lease, [event('p1', 'r', 20)]);
    await store.flushed();
    expect(await column('SELECT id FROM pushed_rows'), isEmpty);
    expect(await painted(), ['p1', 'a1']);
  });

  group('a room forgotten after the fetch gets no pushed rows', () {
    test('since_join notice', () async {
      store.noteRelayKey(lease, _relayKey);
      await applyList({'r'});
      store.submit(lease, [
        notice(
          'h1',
          'r',
          300,
          jsonEncode({
            'type': 'history_changed',
            'actor': 'bb' * 32,
            'history': 'since_join',
          }),
        ),
      ]);
      await store.flushed();
      final early = await store.submitPushed(lease, [
        event('p1', 'r', 20),
      ], fetchedAt: now() - 60);
      expect(early.written, isTrue);
      expect(early.accepted, isEmpty);
      final late = await store.submitPushed(lease, [
        event('p2', 'r', 20),
      ], fetchedAt: now() + 60);
      expect(late.accepted, {'p2'});
    });

    test('rejoin notice', () async {
      store.noteRelayKey(lease, _relayKey);
      await applyList({'r'});
      store.submit(lease, [
        event('a1', 'r', 100),
        notice(
          'l1',
          'r',
          120,
          jsonEncode({'type': 'member_left', 'actor': account}),
        ),
        notice(
          'j1',
          'r',
          140,
          jsonEncode({
            'type': 'member_joined',
            'actor': account,
            'target': account,
          }),
        ),
      ]);
      await store.flushed();
      final outcome = await store.submitPushed(lease, [
        event('p1', 'r', 130),
      ], fetchedAt: now() - 60);
      expect(outcome.accepted, isEmpty);
    });

    test('list drop, then the list has the room again', () async {
      await applyList({'r', 's'});
      store.submit(lease, [event('a1', 'r', 100)]);
      await store.flushed();
      final fetched = now() - 60;
      await applyList({'s'});
      await applyList({'r', 's'});
      final outcome = await store.submitPushed(lease, [
        event('p1', 'r', 130),
      ], fetchedAt: fetched);
      expect(outcome.accepted, isEmpty);
      expect(await column("SELECT id FROM events WHERE room = 'r'"), isEmpty);
    });
  });
}
