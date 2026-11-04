import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:sqflite_common_ffi/sqflite_ffi.dart';

import 'package:kuzz/shared/message_store/message_store.dart';
import 'package:kuzz/shared/message_store/room_head.dart';
import 'package:kuzz/shared/message_store/thread_head.dart';

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

  test('a room paint draws pushed rows, with no gap-free start past the '
      'spans; a certain read leaves them out', () async {
    await applyList({'r'});
    store.submit(lease, [event('a1', 'r', 10)]);
    store.extendCoverage(lease, ['r'], 1, 15);
    await store.flushed();
    final outcome = await store.submitPushed(lease, [
      event('p1', 'r', 20),
      // Already stored from the relay: not marked, drawn as before.
      event('a1', 'r', 10),
    ], fetchedAt: now());
    expect(outcome.accepted, {'p1', 'a1'});
    expect(await column('SELECT id FROM pushed_rows'), ['p1']);

    Future<List<Map<String, dynamic>>> paint() async =>
        await store.paintableRoomHead(
          lease,
          room: 'r',
          kinds: const [9],
          limit: 50,
        ) ??
        const [];
    Object? contiguousFrom(List<Map<String, dynamic>> events) {
      final bounds = events.singleWhere((e) => e['kind'] == 39006);
      final content = jsonDecode(bounds['content'] as String) as Map;
      expect(content, contains('contiguous_from'));
      return content['contiguous_from'];
    }

    final painted = await paint();
    expect(
      [
        for (final message in painted)
          if (message['kind'] == 9) message['id'],
      ],
      ['p1', 'a1'],
    );
    expect(contiguousFrom(painted), isNull);
    final certain = await withDb(
      (db) => readRoomHead(
        db,
        room: 'r',
        kinds: const [9],
        limit: 50,
        liveAt: null,
      ),
    );
    expect([for (final row in certain.rows) row['id']], ['a1']);

    // The relay's copy: the row is confirmed and the span vouches again.
    store.submit(lease, [event('p1', 'r', 20)]);
    store.extendCoverage(lease, ['r'], 1, 30);
    await store.flushed();
    expect(await column('SELECT id FROM pushed_rows'), isEmpty);
    expect(contiguousFrom(await paint()), 1 + rowReceiveMarginSecs);
  });

  test('a room paint counts pushed replies into its summaries, the first '
      'reply included; deleted and already counted ones not', () async {
    Map<String, dynamic> reply(
      String id,
      int at, {
      String root = 'root',
      String? parent,
    }) => {
      ...event(id, 'r', at),
      'tags': [
        ['h', 'r'],
        if (parent != null) ['e', root, '', 'root'],
        ['e', parent ?? root, '', 'reply'],
      ],
    };
    final summary = {
      'id': 's-root',
      'kind': 39005,
      'pubkey': 'relay',
      'created_at': 10,
      'tags': [
        ['e', 'root'],
        ['d', 'root'],
        ['h', 'r'],
      ],
      'content': jsonEncode({
        'reply_count': 1,
        'descendant_count': 1,
        'last_reply_at': 10,
        'participants': ['cc' * 32],
      }),
    };
    await applyList({'r'});
    store.submit(lease, [
      event('root', 'r', 5),
      event('root2', 'r', 6),
      reply('a1', 10),
      summary,
    ]);
    await store.flushed();
    await store.submitPushed(lease, [
      // Older than the summary's last reply: already counted.
      reply('p0', 8),
      reply('p1', 20),
      reply('p2', 25, parent: 'p1'),
      reply('p3', 30),
      reply('q1', 40, root: 'root2'),
    ], fetchedAt: now());
    // The author deleted p3.
    store.submit(lease, [
      {
        ...event('del', 'r', 31),
        'kind': 5,
        'tags': [
          ['h', 'r'],
          ['e', 'p3'],
        ],
      },
    ]);
    await store.flushed();

    Map<String, Map> summaries(List<Map<String, dynamic>> events) => {
      for (final e in events)
        if (e['kind'] == 39005)
          (e['tags'] as List).firstWhere((t) => t[0] == 'e')[1] as String:
              jsonDecode(e['content'] as String) as Map,
    };
    final painted = summaries(
      (await store.paintableRoomHead(
        lease,
        room: 'r',
        kinds: const [9],
        limit: 50,
      ))!,
    );
    expect(painted['root']!['reply_count'], 2);
    expect(painted['root']!['descendant_count'], 3);
    expect(painted['root']!['last_reply_at'], 25);
    expect(painted['root']!['participants'], ['cc' * 32, 'bb' * 32]);
    expect(painted['root2']!['reply_count'], 1);
    expect(painted['root2']!['descendant_count'], 1);
    expect(painted['root2']!['last_reply_at'], 40);

    // Paint only: the certain read and the summaries table stay the relay's.
    final certain = await withDb(
      (db) => readRoomHead(
        db,
        room: 'r',
        kinds: const [9],
        limit: 50,
        liveAt: null,
      ),
    );
    final certainSummaries = summaries(certain.summaries);
    expect(certainSummaries.keys, ['root']);
    expect(certainSummaries['root']!['reply_count'], 1);
    expect(await column('SELECT root FROM summaries'), ['root']);
  });

  test('a thread paint draws pushed replies; a certain read leaves them '
      'out', () async {
    Map<String, dynamic> reply(String id, int at) => {
      ...event(id, 'r', at),
      'tags': [
        ['h', 'r'],
        ['e', 'root', '', 'reply'],
      ],
    };
    await applyList({'r'});
    store.submit(lease, [event('root', 'r', 5), reply('a1', 10)]);
    await store.flushed();
    await store.submitPushed(lease, [reply('p1', 20)], fetchedAt: now());
    expect(await column('SELECT id FROM pushed_rows'), ['p1']);

    final painted = await store.paintableThreadHead(
      lease,
      room: 'r',
      root: 'root',
      kinds: const [9],
      limit: 50,
    );
    expect(
      [
        for (final message in painted!)
          if (message['kind'] == 9) message['id'],
      ],
      ['p1', 'a1'],
    );
    final certainRows = await withDb(
      (db) => readThreadHead(
        db,
        room: 'r',
        root: 'root',
        kinds: const [9],
        limit: 50,
        liveAt: null,
      ),
    );
    expect([for (final row in certainRows.rows) row['id']], ['a1']);
  });

  test(
    'paints say when each pushed row was fetched, the first fetch',
    () async {
      Map<String, dynamic> reply(String id, int at) => {
        ...event(id, 'r', at),
        'tags': [
          ['h', 'r'],
          ['e', 'root', '', 'reply'],
        ],
      };
      Map<dynamic, dynamic> boundsOf(
        List<Map<String, dynamic>> events,
        int kind,
      ) =>
          jsonDecode(
                events.singleWhere((e) => e['kind'] == kind)['content']
                    as String,
              )
              as Map;
      await applyList({'r'});
      store.submit(lease, [event('root', 'r', 5)]);
      await store.flushed();
      await store.submitPushed(lease, [
        event('p1', 'r', 20),
        reply('p2', 21),
      ], fetchedAt: 700);
      await store.submitPushed(lease, [event('p1', 'r', 20)], fetchedAt: 900);

      final room = await store.paintableRoomHead(
        lease,
        room: 'r',
        kinds: const [9],
        limit: 50,
      );
      expect(boundsOf(room!, 39006)['pushed_at'], {'p1': 700});
      final thread = await store.paintableThreadHead(
        lease,
        room: 'r',
        root: 'root',
        kinds: const [9],
        limit: 50,
      );
      expect(boundsOf(thread!, 39007)['pushed_at'], {'p2': 700});
    },
  );

  test('a file of an older build gains fetched_at, its rows without', () async {
    final path = await store.pathOf(lease);
    final old = await databaseFactoryFfi.openDatabase(
      path,
      options: OpenDatabaseOptions(
        version: messageStoreSchemaVersion,
        singleInstance: false,
        onCreate: (db, _) =>
            db.execute('CREATE TABLE pushed_rows(id TEXT PRIMARY KEY)'),
      ),
    );
    await old.close();

    await applyList({'r'});
    await store.submitPushed(lease, [event('p1', 'r', 20)], fetchedAt: 800);
    await store.submitPushed(lease, [event('p2', 'r', 21)], fetchedAt: 800);
    await withDb(
      (db) => db.rawUpdate(
        "UPDATE pushed_rows SET fetched_at = NULL WHERE id = 'p1'",
      ),
    );
    final painted = await store.paintableRoomHead(
      lease,
      room: 'r',
      kinds: const [9],
      limit: 50,
    );
    final bounds = painted!.singleWhere((e) => e['kind'] == 39006);
    expect((jsonDecode(bounds['content'] as String) as Map)['pushed_at'], {
      'p1': null,
      'p2': 800,
    });
  });

  test('a stored root reads by id, pushed or deleted', () async {
    await applyList({'r'});
    store.submit(lease, [
      event('root', 'r', 5),
      event('gone', 'r', 6),
      {
        'id': 'del',
        'kind': 9005,
        'pubkey': _relayKey,
        'created_at': 7,
        'tags': [
          ['h', 'r'],
          ['e', 'gone'],
        ],
        'content': '',
      },
    ]);
    await store.flushed();
    await store.submitPushed(lease, [
      event('pushed', 'r', 8),
    ], fetchedAt: now());

    final root = await store.paintableEvent(lease, room: 'r', id: 'root');
    expect(root?.event['content'], 'root');
    expect(root?.deleted, isFalse);
    final pushed = await store.paintableEvent(lease, room: 'r', id: 'pushed');
    expect(pushed?.event['id'], 'pushed');
    expect(
      (await store.paintableEvent(lease, room: 'r', id: 'gone'))?.deleted,
      isTrue,
    );
    expect(
      await store.paintableEvent(lease, room: 'other', id: 'root'),
      isNull,
    );
    expect(await store.paintableEvent(lease, room: 'r', id: 'missing'), isNull);
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
