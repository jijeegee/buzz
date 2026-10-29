import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:sqflite_common_ffi/sqflite_ffi.dart';

import 'package:kuzz/shared/message_store/coverage.dart';
import 'package:kuzz/shared/message_store/message_store.dart';
import 'package:kuzz/shared/relay/nostr_models.dart';
import 'package:kuzz/shared/relay/room_bundle_coverage.dart';

const _account =
    'aa00000000000000000000000000000000000000000000000000000000000001';
const _relay = 'wss://relay.example';

/// A boundary after every span in these tests.
const _later = 1 << 40;

Map<String, dynamic> _event(String id, int at) => {
  'id': id,
  'kind': 9,
  'pubkey': 'p1',
  'created_at': at,
  'tags': [
    ['h', 'r1'],
  ],
  'content': '',
};

NostrFilter _bundle(List<String> rooms) => NostrFilter(
  kinds: EventKind.channelEventKinds,
  tags: {'#h': rooms},
  limit: 0,
);

void main() {
  sqfliteFfiInit();
  late Directory dir;
  late MessageStore store;
  late MessageStoreLease lease;

  setUp(() async {
    dir = await Directory.systemTemp.createTemp('message_store_coverage');
    store = MessageStore(
      factory: () => databaseFactoryFfi,
      directory: () async => dir.path,
    );
    lease = store.lease(_relay, _account)!;
  });

  tearDown(() async {
    await store.forget(_relay, _account);
    await dir.delete(recursive: true);
  });

  Future<List<Map<String, Object?>>> rows(String sql) async {
    // A second connection: closing it must not close the store's.
    final db = await databaseFactoryFfi.openDatabase(
      await store.pathOf(lease),
      options: OpenDatabaseOptions(singleInstance: false),
    );
    try {
      return await db.rawQuery(sql);
    } finally {
      await db.close();
    }
  }

  test('merging follows the shared fixture', () {
    final fixture =
        jsonDecode(
              File(
                '../test-fixtures/message-store-coverage.json',
              ).readAsStringSync(),
            )
            as Map<String, dynamic>;
    for (final raw in fixture['cases'] as List) {
      final testCase = raw as Map<String, dynamic>;
      List<(int, int)> spans(Object? list) => [
        for (final pair in list! as List)
          ((pair as List)[0] as int, pair[1] as int),
      ];
      final extend = testCase['extend'] as List;
      final merged = mergeCoverage(
        spans(testCase['segments']),
        extend[0] as int,
        extend[1] as int,
      );
      expect(merged, spans(testCase['expect']), reason: '${testCase['name']}');
      final resume = merged.isEmpty
          ? null
          : merged.map((s) => s.$2).reduce((a, b) => a > b ? a : b);
      expect(resume, testCase['resume'], reason: '${testCase['name']}');
    }
  });

  test('coverage follows its events and is forgotten when lossy', () async {
    store.submit(lease, [_event('a1', 100)]);
    store.extendCoverage(lease, ['r1'], 100, 100);
    store.extendCoverage(lease, ['r1'], 100, 160);
    await store.flushed();
    expect(await store.resumePoints(lease, ['r1', 'r2'], _later), {'r1': 160});
    expect(await rows('SELECT id FROM events'), hasLength(1));

    // One large answer is one input and is kept whole.
    store.submit(lease, [for (var i = 0; i < 5000; i++) _event('b$i', 200)]);
    store.extendCoverage(lease, ['r1'], 160, 300);
    await store.flushed();
    expect(await store.resumePoints(lease, ['r1'], _later), {'r1': 300});
    expect(await rows('SELECT id FROM events'), hasLength(5001));

    // Inputs beyond the queue limit are dropped: nothing is known complete.
    for (var i = 0; i < 4100; i++) {
      store.submit(lease, [_event('c$i', 400)]);
    }
    store.extendCoverage(lease, ['r1'], 300, 500);
    await store.flushed();
    expect(await store.resumePoints(lease, ['r1'], _later), isEmpty);
    expect(await rows("SELECT value FROM sync_state WHERE key = 'lossy'"), [
      {'value': '1'},
    ]);
  });

  test('no file means no resume points and none is created', () async {
    expect(await store.resumePoints(lease, ['r1'], _later), isEmpty);
    expect(File(await store.pathOf(lease)).existsSync(), isFalse);
  });

  group('RoomBundleCoverage', () {
    late int now;
    late List<Map<String, dynamic>> requests;
    late List<List<Map<String, dynamic>>> pages;
    late RoomBundleCoverage coverage;

    setUp(() {
      now = 10000;
      requests = [];
      pages = [];
      coverage = RoomBundleCoverage(
        scope: () => (store: store, lease: lease),
        nowSecs: () => now,
        query: (filters) async {
          requests.add(filters.single);
          final page = pages.isEmpty
              ? <Map<String, dynamic>>[]
              : pages.removeAt(0);
          store.submit(lease, page);
          return page;
        },
      );
    });

    test('a first EOSE opens coverage without asking the relay', () async {
      final sub = Object();
      coverage.noteEose(sub, _bundle(['r1']), 1);
      await store.flushed();
      await Future<void>.delayed(Duration.zero);
      expect(requests, isEmpty);
      expect(await store.resumePoints(lease, ['r1'], _later), {'r1': 10000});
      // The REQ's own span is not where a catch-up resumes.
      expect(await store.resumePoints(lease, ['r1'], 10000), isEmpty);

      // Events extend at most once a minute, only on the same connection.
      now = 10030;
      coverage.noteEvent(sub, _bundle(['r1']), 1);
      now = 10070;
      coverage.noteEvent(sub, _bundle(['r1']), 2);
      await store.flushed();
      expect(await store.resumePoints(lease, ['r1'], _later), {'r1': 10000});
      coverage.noteEvent(sub, _bundle(['r1']), 1);
      await store.flushed();
      expect(await store.resumePoints(lease, ['r1'], _later), {'r1': 10070});
    });

    test('an EOSE settles once its catch-up has ended', () async {
      store.extendCoverage(lease, ['r1'], 9000, 10000);
      await store.flushed();
      now = 20000;
      final answer = Completer<List<Map<String, dynamic>>>();
      coverage = RoomBundleCoverage(
        scope: () => (store: store, lease: lease),
        nowSecs: () => now,
        query: (filters) => answer.future,
      );
      final sub = Object();
      expect(coverage.settledOf(sub, 1), isNull);
      coverage.noteEose(sub, _bundle(['r1']), 1);
      var settled = false;
      unawaited(coverage.settledOf(sub, 1)!.then((_) => settled = true));
      // Another connection's REQ is not live.
      expect(coverage.settledOf(sub, 2), isNull);
      await Future<void>.delayed(const Duration(milliseconds: 50));
      expect(settled, isFalse);
      answer.complete(const []);
      await Future<void>.delayed(const Duration(milliseconds: 50));
      expect(settled, isTrue);
    });

    test('a later EOSE catches the store up by keyset pages', () async {
      store.extendCoverage(lease, ['r1'], 9000, 10000);
      await store.flushed();
      now = 20000;
      pages = [
        [for (var i = 0; i < storeCatchUpPageLimit; i++) _event('p$i', 19000)],
        [_event('q1', 12000)],
      ];
      final done = await coverage.catchUp(
        (store: store, lease: lease),
        ['r1', 'r2'],
        now,
      );
      expect(done, isTrue);
      expect(requests, hasLength(2));
      expect(requests.first['#h'], ['r1']);
      expect(requests.first['since'], 10000 - storeResumeLookbackSecs);
      // The first page is not cut at this device's clock.
      expect(requests.first.containsKey('until'), isFalse);
      expect(requests.first.containsKey('before_id'), isFalse);
      expect(requests.last['until'], 19000);
      expect(requests.last['before_id'], 'p${storeCatchUpPageLimit - 1}');
      await store.flushed();
      expect(await store.resumePoints(lease, ['r1', 'r2'], _later), {
        'r1': 20000,
      });
    });

    test('a switch during the catch-up records nothing', () async {
      store.extendCoverage(lease, ['r1'], 9000, 10000);
      await store.flushed();
      var signedIn = true;
      coverage = RoomBundleCoverage(
        scope: () => signedIn ? (store: store, lease: lease) : null,
        nowSecs: () => now,
        query: (filters) async {
          requests.add(filters.single);
          // The account changes while the page is on its way.
          signedIn = false;
          return const <Map<String, dynamic>>[];
        },
      );
      final done = await coverage.catchUp(
        (store: store, lease: lease),
        ['r1'],
        20000,
      );
      expect(done, isFalse);
      expect(requests, hasLength(1));
      await store.flushed();
      expect(await store.resumePoints(lease, ['r1'], _later), {'r1': 10000});
    });

    test('a gap too large is left alone', () async {
      store.extendCoverage(lease, ['r1'], 9000, 10000);
      await store.flushed();
      pages = [
        for (var p = 0; p < storeCatchUpMaxPages; p++)
          [
            for (var i = 0; i < storeCatchUpPageLimit; i++)
              _event('p$p-$i', 19000 - p),
          ],
      ];
      final done = await coverage.catchUp(
        (store: store, lease: lease),
        ['r1'],
        20000,
      );
      expect(done, isFalse);
      expect(requests, hasLength(storeCatchUpMaxPages));
      await store.flushed();
      expect(await store.resumePoints(lease, ['r1'], _later), {'r1': 10000});
    });
  });
}
