import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/shared/message_db/coverage.dart';
import 'package:kuzz/shared/message_db/message_db.dart';
import 'package:kuzz/shared/message_db/message_reads.dart';
import 'package:kuzz/shared/message_db/message_store.dart';
import 'package:kuzz/shared/relay/relay.dart';
import 'package:sqlite3/sqlite3.dart';

import 'message_db_test_support.dart';

void main() {
  late Directory dir;
  late Database db;

  setUp(() {
    dir = Directory.systemTemp.createTempSync('message_reads_test');
    db = openMessageDb('${dir.path}/account.db');
  });

  tearDown(() {
    db.close();
    dir.deleteSync(recursive: true);
  });

  CoverageCursor at(int createdAt, [String id = '']) =>
      CoverageCursor(createdAt, id);

  CoverageClaim claim(
    CoverageCursor lo,
    CoverageCursor hi, {
    String stream = CoverageStream.main,
  }) => CoverageClaim(
    channelId: testChannel,
    stream: stream,
    range: CoverageRange(lo, hi),
  );

  void write(Iterable<NostrEvent> events, {List<CoverageClaim>? claims}) =>
      applyMessageWrite(
        db,
        MessageDbWrite(rows: rowsOf(events), claims: claims ?? const []),
      );

  List<String> ids(StoredTimelinePage? page) => [
    for (final row in page!.rows) row.id,
  ];

  group('newest page', () {
    test('null when nothing complete is stored', () {
      write([testEvent('a', createdAt: 10)]);
      expect(readNewestStoredPage(db, testChannel), isNull);
    });

    test('reads the newest range down to its hole', () {
      write(
        [
          testEvent('old', createdAt: 10),
          testEvent('mid', createdAt: 20),
          testEvent('new1', createdAt: 30),
          testEvent('new2', createdAt: 40),
        ],
        claims: [
          claim(CoverageCursor.start, at(10, 'old')),
          claim(at(30, 'new1'), at(40, 'new2')),
        ],
      );

      final page = readNewestStoredPage(db, testChannel);
      expect(ids(page), ['new2', 'new1']);
      expect(page!.reachesStart, isFalse);
      expect(page.reachesLive, isFalse);
    });

    test('relay order: newest first, ascending id within a second', () {
      write(
        [
          testEvent('b', createdAt: 10),
          testEvent('a', createdAt: 10),
          testEvent('c', createdAt: 11),
        ],
        claims: [claim(CoverageCursor.start, at(11, 'c'))],
      );
      final page = readNewestStoredPage(db, testChannel);
      expect(ids(page), ['c', 'a', 'b']);
      expect(page!.reachesStart, isTrue);
    });

    test('a live range joining the range reaches live', () {
      write(
        [testEvent('a', createdAt: 10), testEvent('b', createdAt: 50)],
        claims: [claim(CoverageCursor.start, at(20))],
      );

      final joined = readNewestStoredPage(db, testChannel, liveFrom: 20);
      expect(ids(joined), ['b', 'a']);
      expect(joined!.reachesLive, isTrue);

      // A live range opened later leaves a hole: only rows above it.
      final apart = readNewestStoredPage(db, testChannel, liveFrom: 40);
      expect(ids(apart), ['b']);
      expect(apart!.reachesStart, isFalse);
    });

    test('an empty room known from its start is an empty page', () {
      write(const [], claims: [claim(CoverageCursor.start, at(30))]);
      final page = readNewestStoredPage(db, testChannel);
      expect(page, isNotNull);
      expect(page!.rows, isEmpty);
      expect(page.reachesStart, isTrue);
    });

    test('deleted rows are left out; overlays come along', () {
      write(
        [
          testEvent('kept', createdAt: 10),
          testEvent('gone', createdAt: 11),
          testOverlay('like', EventKind.reaction, 'kept', createdAt: 12),
          testOverlay('del', EventKind.deletion, 'gone', createdAt: 13),
        ],
        claims: [claim(CoverageCursor.start, at(13))],
      );
      final page = readNewestStoredPage(db, testChannel);
      expect(ids(page), ['kept']);
      expect([for (final o in page!.overlays) o.id], ['like']);
    });

    test('limit cuts the page and keeps reachesStart false', () {
      write(
        [for (var i = 0; i < 5; i++) testEvent('m$i', createdAt: 10 + i)],
        claims: [claim(CoverageCursor.start, at(20))],
      );
      final page = readNewestStoredPage(db, testChannel, limit: 3);
      expect(ids(page), ['m4', 'm3', 'm2']);
      expect(page!.reachesStart, isFalse);
    });
  });

  group('older page', () {
    setUp(() {
      write(
        [
          testEvent('a', createdAt: 10),
          testEvent('b', createdAt: 20),
          testEvent('c', createdAt: 30),
          testEvent('d', createdAt: 40),
        ],
        claims: [
          claim(CoverageCursor.start, at(20, 'b')),
          claim(at(30, 'c'), at(40, 'd')),
        ],
      );
    });

    test('reads down to the end of the range holding the cursor', () {
      final page = readOlderStoredPage(db, testChannel, at(40, 'd'));
      expect(ids(page), ['c']);
      expect(page!.reachesStart, isFalse);
    });

    test('null at a hole: the rows below come from the server', () {
      expect(readOlderStoredPage(db, testChannel, at(30, 'c')), isNull);
    });

    test('an empty page at the room start', () {
      final page = readOlderStoredPage(db, testChannel, at(10, 'a'));
      expect(page, isNotNull);
      expect(page!.rows, isEmpty);
      expect(page.reachesStart, isTrue);
    });

    test('below the hole reads the older range', () {
      final page = readOlderStoredPage(db, testChannel, at(20, 'b'));
      expect(ids(page), ['a']);
      expect(page!.reachesStart, isTrue);
    });
  });

  test('stored through a message time', () {
    write(const [], claims: [claim(at(10), at(30))]);
    expect(isStoredThroughIn(db, testChannel, 30), isTrue);
    expect(isStoredThroughIn(db, testChannel, 31), isFalse);
    expect(isStoredThroughIn(db, testChannel, 31, liveFrom: 31), isTrue);
    expect(isStoredThroughIn(db, testChannel, 31, liveFrom: 32), isFalse);
  });

  test('rows before a time: main rows or one thread', () {
    write([
      testEvent('root', createdAt: 10),
      testReply('reply', 'root', createdAt: 11),
      testEvent('later', createdAt: 50),
    ]);
    expect(
      [for (final e in readStoredRowsBefore(db, testChannel, before: 20)) e.id],
      ['root'],
    );
    expect(
      [
        for (final e in readStoredRowsBefore(
          db,
          testChannel,
          before: 20,
          rootId: 'root',
        ))
          e.id,
      ],
      ['reply', 'root'],
    );
  });

  group('thread pages', () {
    setUp(() {
      write([
        testEvent('root', createdAt: 5),
        testEvent('main', createdAt: 25),
        testReply('r1', 'root', createdAt: 10),
        testReply('r2', 'root', createdAt: 20),
        testReply('r3', 'root', createdAt: 30),
        testReply('r4', 'root', createdAt: 40),
        testOverlay('like', EventKind.reaction, 'r4', createdAt: 41),
      ]);
    });

    test('main coverage does not cover a thread', () {
      write(const [], claims: [claim(CoverageCursor.start, at(50))]);
      expect(readNewestStoredPage(db, testChannel, rootId: 'root'), isNull);
      expect(ids(readNewestStoredPage(db, testChannel)), ['main', 'root']);
    });

    test('thread and window ranges merge; rows are replies only', () {
      write(
        const [],
        claims: [
          claim(
            CoverageCursor.start,
            at(20, 'r2'),
            stream: CoverageStream.thread('root'),
          ),
          claim(
            at(20, 'r2'),
            at(40, 'r4'),
            stream: CoverageStream.threadWindow('root'),
          ),
        ],
      );
      final page = readNewestStoredPage(db, testChannel, rootId: 'root');
      expect(ids(page), ['r4', 'r3', 'r2', 'r1']);
      expect(page!.reachesStart, isTrue);
      expect(page.reachesLive, isFalse);
      expect([for (final o in page.overlays) o.id], ['like']);
    });

    test('the live range and all-stream ranges cover a thread', () {
      write(
        const [],
        claims: [
          claim(CoverageCursor.start, at(25), stream: CoverageStream.all),
        ],
      );
      final page = readNewestStoredPage(
        db,
        testChannel,
        rootId: 'root',
        liveFrom: 26,
      );
      expect(ids(page), ['r4', 'r3', 'r2', 'r1']);
      expect(page!.reachesLive, isTrue);
    });

    test('older and newer pages stay inside one range', () {
      write(
        const [],
        claims: [
          claim(
            at(20, 'r2'),
            at(40, 'r4'),
            stream: CoverageStream.threadWindow('root'),
          ),
        ],
      );
      final older = readOlderStoredPage(
        db,
        testChannel,
        at(40, 'r4'),
        rootId: 'root',
      );
      expect(ids(older), ['r3', 'r2']);
      expect(older!.reachesStart, isFalse);
      expect(
        readOlderStoredPage(db, testChannel, at(20, 'r2'), rootId: 'root'),
        isNull,
      );

      final newer = readNewerStoredPage(
        db,
        testChannel,
        at(20, 'r2'),
        rootId: 'root',
      );
      expect(ids(newer), ['r3', 'r4']);
      expect(newer!.reachesLive, isFalse);
      // Above the range: not known complete.
      expect(
        readNewerStoredPage(db, testChannel, at(40, 'r4'), rootId: 'root'),
        isNull,
      );
    });

    test('a newer page reaching the live range reaches live', () {
      write(
        const [],
        claims: [
          claim(
            CoverageCursor.start,
            at(29),
            stream: CoverageStream.thread('root'),
          ),
        ],
      );
      final newer = readNewerStoredPage(
        db,
        testChannel,
        at(20, 'r2'),
        rootId: 'root',
        liveFrom: 30,
      );
      expect(ids(newer), ['r3', 'r4']);
      expect(newer!.reachesLive, isTrue);

      final cut = readNewerStoredPage(
        db,
        testChannel,
        at(10, 'r1'),
        rootId: 'root',
        liveFrom: 30,
        limit: 2,
      );
      expect(ids(cut), ['r2', 'r3']);
      expect(cut!.reachesLive, isFalse);
    });
  });

  group('first stored row after a read marker', () {
    bool all(NostrEvent row) => true;

    setUp(() {
      write([
        testEvent('before', createdAt: 10),
        testEvent('mine', createdAt: 20),
        testEvent('theirs', createdAt: 30),
        testReply('reply', 'before', createdAt: 25),
      ]);
    });

    test('the first counting main row inside the range holding the marker', () {
      write(const [], claims: [claim(CoverageCursor.start, at(30, 'theirs'))]);
      final first = readFirstStoredAfter(
        db,
        testChannel,
        15,
        (row) => row.id != 'mine',
      );
      expect(first.known, isTrue);
      expect(first.row?.id, 'theirs');
    });

    test('rows at the marker second are read', () {
      write(const [], claims: [claim(CoverageCursor.start, at(30, 'theirs'))]);
      expect(readFirstStoredAfter(db, testChannel, 20, all).row?.id, 'theirs');
    });

    test('none counting: known only when the range reaches live', () {
      write(const [], claims: [claim(CoverageCursor.start, at(30, 'theirs'))]);
      bool none(NostrEvent row) => false;
      expect(readFirstStoredAfter(db, testChannel, 15, none).known, isFalse);
      final live = readFirstStoredAfter(
        db,
        testChannel,
        15,
        none,
        liveFrom: 30,
      );
      expect(live.known, isTrue);
      expect(live.row, isNull);
    });

    test('a marker outside every stored range is unknown', () {
      write(const [], claims: [claim(at(25), at(30, 'theirs'))]);
      final first = readFirstStoredAfter(db, testChannel, 15, all);
      expect(first.known, isFalse);
      expect(first.row, isNull);
    });

    test('created_at of a stored main row', () {
      expect(readStoredCreatedAt(db, testChannel, 'theirs'), 30);
      expect(readStoredCreatedAt(db, testChannel, 'reply'), isNull);
      expect(readStoredCreatedAt(db, testChannel, 'missing'), isNull);
    });
  });

  group('store reader', () {
    const scope = MessageScope(relayUrl: 'https://one.example', pubkey: 'a');
    late MessageStore store;

    setUp(() {
      store = MessageStore(
        () async => '${dir.path}/messages',
        rootPath: '${dir.path}/messages',
      );
    });

    tearDown(() => store.close());

    test('null before the file exists; sees later writes', () async {
      expect(store.reader(scope), isNull);
      await store.write(scope, MessageDbWrite(rows: rowsOf([testEvent('a')])));
      final reader = store.reader(scope);
      expect(reader, isNotNull);
      expect(storedIds(reader!), ['a']);

      await store.write(scope, MessageDbWrite(rows: rowsOf([testEvent('b')])));
      expect(storedIds(store.reader(scope)!), ['a', 'b']);
    });

    test('a clear closes the reader and removes the data', () async {
      await store.write(scope, MessageDbWrite(rows: rowsOf([testEvent('a')])));
      expect(store.reader(scope), isNotNull);
      await store.clear(scope);
      expect(store.reader(scope), isNull);
    });

    test('read-only: the reader cannot write', () async {
      await store.write(scope, MessageDbWrite(rows: rowsOf([testEvent('a')])));
      expect(
        () => store.reader(scope)!.execute('DELETE FROM events'),
        throwsA(isA<SqliteException>()),
      );
    });
  });
}
