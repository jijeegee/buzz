import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/shared/message_db/coverage.dart';
import 'package:kuzz/shared/message_db/message_db.dart';
import 'package:kuzz/shared/relay/relay.dart';
import 'package:sqlite3/sqlite3.dart';

import 'message_db_test_support.dart';

void main() {
  late Directory dir;
  late String path;
  late Database db;

  setUp(() {
    dir = Directory.systemTemp.createTempSync('message_db_test');
    path = '${dir.path}/scope/account.db';
    db = openMessageDb(path);
  });

  tearDown(() {
    db.close();
    dir.deleteSync(recursive: true);
  });

  void write(Iterable<NostrEvent> events, {List<CoverageClaim>? claims}) =>
      applyMessageWrite(
        db,
        MessageDbWrite(rows: rowsOf(events), claims: claims ?? const []),
      );

  group('rows', () {
    test('classifies top-level rows, replies and overlays', () {
      write([
        testEvent('top'),
        testReply('reply', 'top'),
        testOverlay('edit', EventKind.streamMessageEdit, 'top'),
        testOverlay('like', EventKind.reaction, 'top'),
        testEvent('join', kind: EventKind.huddleParticipantJoined),
      ]);

      final rows = {
        for (final row in db.select('SELECT id, stream, target_id FROM events'))
          row['id']: (row['stream'], row['target_id']),
      };
      expect(rows, {
        'top': ('', null),
        'reply': ('top', null),
        'edit': (null, 'top'),
        'like': (null, 'top'),
        'join': (null, null),
      });
    });

    test('a duplicate id is stored once and keeps the first copy', () {
      write([testEvent('a', content: 'first')]);
      write([testEvent('a', content: 'second'), testEvent('a')]);

      expect(storedIds(db), ['a']);
      expect(storedRow(db, 'a')?.content, 'first');
    });

    test('window bounds and events without a room are not stored', () {
      write([
        testEvent('bounds', kind: EventKind.channelWindowBounds),
        NostrEvent(
          id: 'roomless',
          pubkey: 'p',
          createdAt: 1,
          kind: EventKind.streamMessage,
          tags: const [],
          content: '',
        ),
      ]);

      expect(storedIds(db), isEmpty);
    });
  });

  group('deletions', () {
    test('a deletion after its target blanks it and its edits', () {
      write([
        testEvent('msg', content: 'secret'),
        testOverlay(
          'edit',
          EventKind.streamMessageEdit,
          'msg',
          content: 'secret v2',
        ),
        testOverlay('like', EventKind.reaction, 'msg'),
      ]);
      write([testOverlay('del', EventKind.deletion, 'msg')]);

      expect(storedRow(db, 'msg'), (deleted: true, content: '', id: 'msg'));
      expect(storedRow(db, 'edit'), (deleted: true, content: '', id: 'edit'));
      // Reactions carry no message text; the read path ignores them anyway.
      expect(storedRow(db, 'like')?.deleted, isFalse);
      expect(
        db.select('SELECT deletion_id FROM deletions WHERE target_id = ?', [
          'msg',
        ]).single['deletion_id'],
        'del',
      );
      // Deleted rows leave the timeline index.
      expect(
        db.select(
          "SELECT id FROM events WHERE channel_id = ? AND stream = '' "
          'AND deleted = 0',
          [testChannel],
        ),
        isEmpty,
      );
    });

    test('a target that arrives after its deletion is stored deleted', () {
      write([testOverlay('del', EventKind.nip29DeleteEvent, 'msg')]);
      write([
        testEvent('msg', content: 'secret'),
        testOverlay(
          'edit',
          EventKind.streamMessageEdit,
          'msg',
          content: 'secret v2',
        ),
      ]);

      expect(storedRow(db, 'msg'), (deleted: true, content: '', id: 'msg'));
      expect(storedRow(db, 'edit')?.deleted, isTrue);
      expect(storedRow(db, 'edit')?.content, '');
    });

    test('a deletion and its target in one write', () {
      write([
        testOverlay('del', EventKind.deletion, 'msg'),
        testEvent('msg', content: 'secret'),
      ]);

      expect(storedRow(db, 'msg')?.deleted, isTrue);
    });

    test('deleting a reaction removes it', () {
      write([testOverlay('like', EventKind.reaction, 'msg')]);
      write([testOverlay('unlike', EventKind.deletion, 'like')]);

      expect(storedRow(db, 'like')?.deleted, isTrue);
    });
  });

  group('thread summaries', () {
    NostrEvent summary(String id, int createdAt) => testOverlay(
      id,
      EventKind.channelThreadSummary,
      'root',
      createdAt: createdAt,
      content: '{"reply_count":$createdAt}',
    );

    List<String> summaries() => [
      for (final row in db.select('SELECT id FROM events WHERE kind = ?', [
        EventKind.channelThreadSummary,
      ]))
        row['id'] as String,
    ];

    test('the newest summary per root wins, whatever the arrival order', () {
      write([summary('s2', 200)]);
      write([summary('s1', 100)]);
      expect(summaries(), ['s2']);

      write([summary('s3', 300)]);
      expect(summaries(), ['s3']);
    });

    test('within one second the later arrival wins', () {
      write([summary('a', 100)]);
      write([summary('b', 100)]);

      expect(summaries(), ['b']);
    });

    test('a repeated summary is ignored', () {
      write([summary('a', 100)]);
      write([summary('a', 100)]);

      expect(summaries(), ['a']);
    });
  });

  group('coverage', () {
    CoverageClaim claim(
      int lo,
      int hi, {
      String stream = CoverageStream.main,
    }) => CoverageClaim(
      channelId: testChannel,
      stream: stream,
      range: CoverageRange(CoverageCursor(lo, 'x'), CoverageCursor(hi, 'x')),
    );

    List<(int, int)> ranges([String stream = CoverageStream.main]) => [
      for (final range in readCoverage(db, testChannel, stream))
        (range.lo.createdAt, range.hi.createdAt),
    ];

    test('claims merge, split and leave holes in one stream', () {
      write([], claims: [claim(10, 20)]);
      write([], claims: [claim(40, 50)]);
      expect(ranges(), [(10, 20), (40, 50)]);

      write([], claims: [claim(20, 30)]);
      expect(ranges(), [(10, 30), (40, 50)]);

      write([], claims: [claim(30, 40)]);
      expect(ranges(), [(10, 50)]);
    });

    test('streams and rooms are separate', () {
      write([], claims: [claim(10, 20)]);
      write([], claims: [claim(20, 30, stream: CoverageStream.thread('r'))]);

      expect(ranges(), [(10, 20)]);
      expect(ranges(CoverageStream.thread('r')), [(20, 30)]);
      expect(readCoverage(db, 'other-room', CoverageStream.main), isEmpty);
    });

    test('arriving events never create coverage', () {
      write([testEvent('a', createdAt: 10), testEvent('b', createdAt: 20)]);

      expect(db.select('SELECT * FROM coverage'), isEmpty);
    });
  });

  group('sync state', () {
    int? watermark() =>
        db.select('SELECT watermark FROM sync_state WHERE origin = ?', [
              'https://a',
            ]).firstOrNull?['watermark']
            as int?;

    test('the watermark only moves forward and is kept per origin', () {
      applyMessageWrite(
        db,
        const MessageDbWrite(
          sync: SyncStateUpdate(origin: 'https://a', watermark: 200),
        ),
      );
      applyMessageWrite(
        db,
        const MessageDbWrite(
          sync: SyncStateUpdate(origin: 'https://a', watermark: 100),
        ),
      );
      applyMessageWrite(
        db,
        const MessageDbWrite(
          sync: SyncStateUpdate(origin: 'https://b', watermark: 50),
        ),
      );

      expect(watermark(), 200);
    });

    test('a failed write stores neither its events nor its watermark', () {
      db.execute(
        'CREATE TRIGGER fail_bad BEFORE INSERT ON events '
        "WHEN NEW.id = 'bad' BEGIN SELECT RAISE(ABORT, 'boom'); END",
      );

      expect(
        () => applyMessageWrite(
          db,
          MessageDbWrite(
            rows: rowsOf([testEvent('good'), testEvent('bad')]),
            liveFrom: const {testChannel: null},
            sync: const SyncStateUpdate(origin: 'https://a', watermark: 500),
          ),
        ),
        throwsA(isA<SqliteException>()),
      );

      expect(storedIds(db), isEmpty);
      expect(watermark(), isNull);
      expect(db.autocommit, isTrue);
    });

    test('live ranges start and end per room', () {
      applyMessageWrite(db, const MessageDbWrite(liveFrom: {'a': 10, 'b': 20}));
      applyMessageWrite(
        db,
        const MessageDbWrite(liveFrom: {'a': null, 'b': 30}),
      );

      expect(
        [
          for (final row in db.select('SELECT * FROM room_state'))
            (row['channel_id'], row['live_from']),
        ],
        [('b', 30)],
      );
    });
  });

  group('file', () {
    test('reopening keeps rows and settings', () {
      write(
        [testEvent('a')],
        claims: [
          CoverageClaim(
            channelId: testChannel,
            stream: CoverageStream.main,
            range: const CoverageRange(
              CoverageCursor.start,
              CoverageCursor(1, 'a'),
            ),
          ),
        ],
      );
      db.close();

      db = openMessageDb(path);

      expect(storedIds(db), ['a']);
      expect(readCoverage(db, testChannel, CoverageStream.main), hasLength(1));
      expect(db.select('PRAGMA user_version').single.values.single, 1);
      expect(db.select('PRAGMA journal_mode').single.values.single, 'wal');
      // 2 = incremental, so a later deletion stage can free pages.
      expect(db.select('PRAGMA auto_vacuum').single.values.single, 2);
    });

    test('an unknown schema version is moved aside for an empty file', () {
      write([testEvent('a')]);
      db.execute('PRAGMA user_version = 99');
      db.close();

      db = openMessageDb(path);

      expect(storedIds(db), isEmpty);
      expect(db.select('PRAGMA user_version').single.values.single, 1);
      final kept = File(path).parent
          .listSync()
          .map((entry) => entry.path)
          .where((name) => name.contains('.v99.'));
      expect(kept, isNotEmpty);
    });

    test('a file that is not a database is moved aside', () {
      db.close();
      for (final suffix in ['', '-wal', '-shm']) {
        final file = File('$path$suffix');
        if (file.existsSync()) file.deleteSync();
      }
      File(path).writeAsStringSync(
        'not a database at all, '
        'just some text that is long enough to fill a header page',
      );

      db = openMessageDb(path);

      expect(storedIds(db), isEmpty);
      expect(
        File(path).parent.listSync().where((e) => e.path.contains('.corrupt.')),
        isNotEmpty,
      );
    });

    test('the reader connection cannot write', () {
      write([testEvent('a')]);
      final reader = openMessageDbReader(path);
      addTearDown(reader.close);

      expect(reader.select('SELECT id FROM events').single['id'], 'a');
      expect(
        () => reader.execute("DELETE FROM events WHERE id = 'a'"),
        throwsA(isA<SqliteException>()),
      );
    });
  });
}
