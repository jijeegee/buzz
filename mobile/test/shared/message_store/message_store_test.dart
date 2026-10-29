import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:sqflite_common_ffi/sqflite_ffi.dart';

import 'package:kuzz/shared/message_store/message_store.dart';
import 'package:kuzz/shared/message_store/stored_event.dart';

const _account =
    'aa00000000000000000000000000000000000000000000000000000000000001';
const _other =
    'bb00000000000000000000000000000000000000000000000000000000000002';
const _relay = 'wss://relay.example';
const _elsewhere = 'wss://other.example';

Map<String, dynamic> _event(String id, int at, {String content = ''}) => {
  'id': id,
  'kind': 9,
  'pubkey': 'p1',
  'created_at': at,
  'tags': [
    ['h', 'r'],
  ],
  'content': content,
};

Map<String, dynamic> _summary(int at, String content) => {
  'id': 's$at',
  'kind': 39005,
  'pubkey': 'relay',
  'created_at': at,
  'tags': [
    ['e', 'r1'],
    ['h', 'room1'],
  ],
  'content': content,
};

void main() {
  sqfliteFfiInit();
  late Directory dir;
  late MessageStore store;

  setUp(() async {
    dir = await Directory.systemTemp.createTemp('message_store');
    store = MessageStore(
      factory: () => databaseFactoryFfi,
      directory: () async => dir.path,
    );
  });

  tearDown(() async {
    await store.forget(_relay, _account);
    await store.forget(_relay, _other);
    await store.forget(_elsewhere, _account);
    await dir.delete(recursive: true);
  });

  Future<List<Map<String, Object?>>> rows(
    MessageStoreLease lease,
    String sql,
  ) async {
    final db = await databaseFactoryFfi.openDatabase(await store.pathOf(lease));
    try {
      return await db.rawQuery(sql);
    } finally {
      await db.close();
    }
  }

  test('classification follows the shared fixture', () {
    final fixture =
        jsonDecode(
              File('../test-fixtures/message-store.json').readAsStringSync(),
            )
            as Map<String, dynamic>;
    for (final raw in fixture['cases'] as List) {
      final testCase = raw as Map<String, dynamic>;
      final name = testCase['name'];
      final expect_ = testCase['expect'] as Map<String, dynamic>;
      final filed = classifyStoredEvent(
        testCase['event'] as Map<String, dynamic>,
      );
      if (expect_['stored'] != true) {
        expect(filed, isNull, reason: '$name: should not be kept');
        continue;
      }
      switch (filed) {
        case StoredSummary():
          expect(expect_['summary'], isTrue, reason: '$name');
          expect(filed.root, expect_['root'], reason: '$name: root');
          expect(filed.room, expect_['room'], reason: '$name: room');
        case StoredRow():
          expect(expect_['summary'] ?? false, isFalse, reason: '$name');
          expect(filed.room, expect_['room'], reason: '$name: room');
          expect(filed.root, expect_['root'], reason: '$name: root');
          expect(filed.broadcast, expect_['broadcast'], reason: '$name');
          expect(filed.targets, expect_['targets'], reason: '$name: targets');
        case null:
          fail('$name: should be kept');
      }
    }
  });

  test('first copy wins, newest summary kept, one file per account', () async {
    final mine = store.lease(_relay, _account)!;
    final theirs = store.lease(_relay, _other)!;
    store.submit(mine, [_event('a1', 10), _summary(20, 'new')]);
    store.submit(mine, [
      _event('a1', 10, content: 'changed'),
      _summary(15, 'old'),
    ]);
    store.submit(theirs, [_event('b1', 1), _event('b2', 2)]);
    await store.flushed();
    final events = await rows(mine, 'SELECT raw FROM events');
    expect(events, hasLength(1));
    expect(events.single['raw'] as String, isNot(contains('changed')));
    final summary = await rows(mine, 'SELECT raw FROM summaries');
    expect(summary.single['raw'] as String, contains('"new"'));
    expect(await rows(theirs, 'SELECT id FROM events'), hasLength(2));
    // A socket and a query to the same server share the file.
    expect(store.lease('https://relay.example/', _account), mine);
  });

  test(
    'sign-out deletes only that account on that server, fences late writes',
    () async {
      final mine = store.lease(_relay, _account)!;
      final theirs = store.lease(_relay, _other)!;
      final elsewhere = store.lease(_elsewhere, _account)!;
      store.submit(mine, [_event('a1', 1)]);
      store.submit(theirs, [_event('b1', 1)]);
      store.submit(elsewhere, [_event('e1', 1)]);
      await store.flushed();
      // The community's relay URL as stored, not normalised.
      await store.forget('https://relay.example/', _account);
      expect(File(await store.pathOf(mine)).existsSync(), isFalse);
      expect(File(await store.pathOf(theirs)).existsSync(), isTrue);
      // The same account on another server keeps its file and keeps recording.
      store.submit(elsewhere, [_event('e2', 2)]);
      await store.flushed();
      expect(await rows(elsewhere, 'SELECT id FROM events'), hasLength(2));
      // An answer to a request sent before the sign-out arrives late.
      store.submit(mine, [_event('a2', 2)]);
      await store.flushed();
      expect(File(await store.pathOf(mine)).existsSync(), isFalse);
      // Signing in again starts a fresh scope.
      final again = store.lease(_relay, _account)!;
      store.submit(again, [_event('a3', 3)]);
      await store.flushed();
      expect(await rows(again, 'SELECT id FROM events'), hasLength(1));
    },
  );
}
