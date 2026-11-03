// Shared rules with desktop (test-fixtures/user-stream.json): the stream's
// catch-up, room_seq and the rest of the live/gated side
// (docs/message-sync.md#user-stream).
import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:math';

import 'package:flutter_test/flutter_test.dart';
import 'package:sqflite_common_ffi/sqflite_ffi.dart';
import 'package:kuzz/features/channels/room_live_router.dart';
import 'package:kuzz/shared/message_store/coverage.dart';
import 'package:kuzz/shared/message_store/message_store.dart';
import 'package:kuzz/shared/relay/nostr_models.dart';
import 'package:kuzz/shared/relay/room_bundle_coverage.dart';
import 'package:kuzz/shared/relay/user_stream.dart';

const _account =
    'aa00000000000000000000000000000000000000000000000000000000000001';
const _relay = 'wss://relay.example';
const _relayKey = 'relay';

final Map<String, dynamic> _fixture =
    jsonDecode(File('../test-fixtures/user-stream.json').readAsStringSync())
        as Map<String, dynamic>;

List<Map<String, dynamic>> _cases(String section) => [
  for (final c in _fixture[section] as List) (c as Map).cast<String, dynamic>(),
];

Map<String, int> _ints(Object? map) => {
  for (final MapEntry(:key, :value) in ((map ?? const {}) as Map).entries)
    key as String: value as int,
};

List<String> _strings(Object? list) => [
  for (final item in (list ?? const []) as List) item as String,
];

List<(int, int)> _spans(Object? list) => [
  for (final pair in (list ?? const []) as List)
    ((pair as List)[0] as int, pair[1] as int),
];

/// The newest segment begun before [liveAt].
(int, int)? _head(List<(int, int)> segments, int liveAt) {
  (int, int)? head;
  for (final segment in segments) {
    if (segment.$1 >= liveAt) continue;
    if (head == null || segment.$2 > head.$2) head = segment;
  }
  return head;
}

Map<String, dynamic> _part1({
  String mode = 'live',
  String epoch = 'e1',
  Map<String, int> rooms = const {},
  bool badKinds = false,
  int createdAt = 50,
}) => {
  'id': 'p1',
  'pubkey': _relayKey,
  'created_at': createdAt,
  'kind': userStreamKind,
  'tags': const [],
  'content': jsonEncode({
    'v': 1,
    'op': 'snapshot',
    'part': 1,
    'parts': 1,
    'mode': mode,
    'epoch': epoch,
    'kinds': badKinds
        ? userStreamFrozenKinds.skip(1).toList()
        : userStreamFrozenKinds,
    'rooms': rooms,
  }),
  'sig': '',
};

Map<String, dynamic> _roomEvent(String id, String room, int at) => {
  'id': id,
  'pubkey': 'p1',
  'created_at': at,
  'kind': 9,
  'tags': [
    ['h', room],
  ],
  'content': '',
  'sig': '',
};

/// A stream with a recording host.
class _Stream {
  _Stream({this.queueEpoch, this.catchUp, this.recordCoverage = true});

  final void Function(String epoch)? queueEpoch;
  final Future<UserStreamCatchUpResult?> Function(
    UserStreamCatchUpJob job,
    bool Function(String id) receivedLive,
  )?
  catchUp;
  final bool recordCoverage;
  final stored = <String>[];
  final rendered = <String>[];
  final coverage = <String, List<(int, int)>>{};
  final timers = <(Duration, void Function())>[];
  var nowMs = 1000000;
  late final stream =
      UserStream(
        UserStreamHost(
          nextSubId: () => 'l-1',
          send: (_) {},
          isActive: (_) => true,
          store: (event) => stored.add(event['id'] as String),
          closeSocket: (_) {},
          scope: () => 'scope',
          readState: (_) async => const {},
          writeState: (_) {},
          chunkRooms: () => const {},
          nowMs: () => nowMs,
          timer: (duration, callback) {
            timers.add((duration, callback));
            return Timer(const Duration(days: 1), () {});
          },
          queueEpoch: queueEpoch ?? (_) {},
          extendCoverage: (rooms, from, to, _) {
            if (!recordCoverage) return;
            for (final room in rooms) {
              coverage.putIfAbsent(room, () => []).add((from, to));
            }
          },
          catchUp: catchUp ?? (_, _) async => null,
        ),
      )..addFrameListener((event, render) {
        if (render) rendered.add(event['id'] as String);
      });

  void send(List<Object?> frame) => stream.onMessage(frame);
}

void main() {
  sqfliteFfiInit();
  late Directory dir;
  late MessageStore store;
  late MessageStoreLease lease;

  setUp(() async {
    dir = await Directory.systemTemp.createTemp('user_stream_catch_up');
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

  Future<T> direct<T>(Future<T> Function(Database db) body) async {
    await store.writeSyncState(lease, {'seeded': '1'});
    await store.flushed();
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

  Future<void> seedEpoch(String epoch) => direct(
    (db) => db.rawInsert(
      'INSERT OR REPLACE INTO sync_state(key, value) VALUES (?, ?)',
      [userStreamEpochKey, epoch],
    ),
  );

  Future<void> seedRow(String room, int seq, int at) => direct(
    (db) => db.rawInsert(
      'INSERT OR REPLACE INTO room_seq(room, seq, at) VALUES (?, ?, ?)',
      [room, seq, at],
    ),
  );

  Future<Map<String, List<int>>> rows() => direct((db) async {
    final found = await db.rawQuery('SELECT room, seq, at FROM room_seq');
    return {
      for (final row in found)
        row['room'] as String: [row['seq'] as int, row['at'] as int],
    };
  });

  Future<String?> storedEpoch() => direct((db) async {
    final found = await db.rawQuery(
      'SELECT value FROM sync_state WHERE key = ?',
      [userStreamEpochKey],
    );
    return found.firstOrNull?['value'] as String?;
  });

  Future<List<List<int>>> segments(String room) => direct((db) async {
    final found = await db.rawQuery(
      'SELECT lo_at, hi_at FROM coverage WHERE room = ? ORDER BY lo_at',
      [room],
    );
    return [
      for (final row in found) [row['lo_at'] as int, row['hi_at'] as int],
    ];
  });

  Future<void> seedSegment(String room, int lo, int hi) async {
    store.extendCoverage(lease, [room], lo, hi);
    await store.flushed();
  }

  /// A `/query` that answers nothing and records the rooms and `since` of
  /// each walk's first page.
  ({
    List<(List<String>, int)> walks,
    Future<Object?> Function(List<Map<String, dynamic>>) query,
  })
  recordingQuery() {
    final walks = <(List<String>, int)>[];
    return (
      walks: walks,
      query: (filters) async {
        final filter = filters.single;
        walks.add((_strings(filter['#h']), filter['since'] as int));
        return const [];
      },
    );
  }

  UserStreamCatchUp runner(
    Future<Object?> Function(List<Map<String, dynamic>>) query,
  ) => UserStreamCatchUp(
    scope: (store: store, lease: lease),
    query: query,
    switched: () => false,
    receivedLive: (_) => false,
    random: Random(1),
  );

  test('a probing walk stores the page itself and reports new rows', () async {
    store.submit(lease, [_roomEvent('old', 'room-a', 10)]);
    await store.flushed();
    Object? zoneFlag;
    final walk = await walkStore(
      scope: (store: store, lease: lease),
      query: (filters) async {
        zoneFlag = Zone.current[storeWalkProbeZoneKey];
        return [
          _roomEvent('old', 'room-a', 10),
          _roomEvent('new', 'room-a', 20),
        ];
      },
      switched: () => false,
      rooms: const ['room-a'],
      since: 0,
      liveAt: 100,
      probe: true,
    );
    expect(zoneFlag, isTrue);
    expect(walk.completed, isTrue);
    expect([for (final e in walk.inserted) e['id']], ['new']);
    expect(await store.storedEventIds(lease, ['old', 'new']), {'old', 'new'});
  });

  group('epoch', () {
    for (final c in _cases('epoch')) {
      test(c['name'], () async {
        if (c['storedEpoch'] case final String epoch) await seedEpoch(epoch);
        for (final MapEntry(:key, :value) in (c['rowsBefore'] as Map).entries) {
          await seedRow(key as String, value[0] as int, value[1] as int);
        }
        final s = _Stream(
          queueEpoch: (epoch) => store.queueStreamEpoch(lease, epoch),
        );
        await s.stream.connected(1);
        final part1 = c['part1'] as Map;
        s.send([
          'EVENT',
          'l-1',
          _part1(
            epoch: part1['epoch'] as String,
            mode: (part1['mode'] as String?) ?? 'live',
            badKinds: part1['accept'] == false,
          ),
        ]);
        final expected = c['expect'] as Map;
        expect(await storedEpoch(), expected['storedEpoch']);
        expect(await rows(), expected['rows']);
      });
    }
  });

  group('snapshot', () {
    for (final c in _cases('snapshot')) {
      test(c['name'], () async {
        final liveAt = c['L'] as int;
        final snapshot = _ints(c['snapshot']);
        final stored = _ints(c['stored']);
        final resume = _ints(c['resume']);
        await seedEpoch('e1');
        for (final MapEntry(:key, :value) in resume.entries) {
          await seedSegment(key, value - 1000, value);
        }
        for (final MapEntry(:key, :value) in stored.entries) {
          await seedRow(key, value, 1);
        }
        final q = recordingQuery();
        final result = await runner(q.query).run(
          UserStreamCatchUpJob(
            mode: c['mode'] as String,
            rooms: snapshot.keys.toList(),
            seqs: snapshot,
            epoch: 'e1',
            relayPubkey: _relayKey,
            liveAt: liveAt,
            captured: null,
          ),
        );
        final expected = c['expect'] as Map;
        expect({
          for (final MapEntry(:key, :value) in result.classes.entries)
            key: value.name,
        }, expected['classes']);
        expect(result.walked.toList()..sort(), expected['walk']);
        final skip = c['mode'] == 'gated'
            ? [
                for (final MapEntry(:key, :value) in result.classes.entries)
                  if (value == UserStreamRoomClass.equal) key,
              ]
            : <String>[];
        expect(skip..sort(), expected['skip']);
        expect(
          result.counts['predictedSkips'] ?? 0,
          (expected['predictedSkips'] as List).length,
        );
        final after = await rows();
        for (final room in _strings(expected['clearedRows'])) {
          expect(after[room]?.first, isNot(stored[room]), reason: room);
        }
        for (final room in _strings(expected['untouchedRows'])) {
          expect(after[room], [stored[room], 1], reason: room);
        }
        final fetched = {for (final walk in q.walks) ...walk.$1};
        for (final room in _strings(expected['notFetched'])) {
          expect(fetched, isNot(contains(room)));
        }
      });
    }
  });

  group('skipCoverage', () {
    for (final c in _cases('skipCoverage')) {
      test(c['name'], () async {
        final liveAt = c['L'] as int;
        final room = c['room'] as String;
        final before = c['before'] as Map;
        final row = before['row'] as List;
        await seedEpoch('e1');
        for (final (lo, hi) in _spans(before['segments'])) {
          await seedSegment(room, lo, hi);
        }
        await seedRow(room, row[0] as int, row[1] as int);
        // The stream's own span at its EOSE.
        await seedSegment(room, liveAt, liveAt);
        final seq = c['verdict'] == 'skip' ? row[0] as int : c['S'] as int;
        final q = recordingQuery();
        await runner(q.query).run(
          UserStreamCatchUpJob(
            mode: c['mode'] as String,
            rooms: [room],
            seqs: {room: seq},
            epoch: 'e1',
            relayPubkey: _relayKey,
            liveAt: liveAt,
            captured: null,
          ),
          audit: false,
        );
        if (c['since'] case final int since) {
          expect(q.walks.single.$2, since);
        }
        final expected = c['expect'] as Map;
        expect(await segments(room), expected['segments']);
        expect((await rows())[room], expected['row']);
      });
    }
  });

  group('connectionSpans', () {
    for (final c in _cases('connectionSpans')) {
      test(c['name'], () async {
        final connectedAt = c['connectedAt'] as int;
        final liveAt = c['L'] as int;
        for (final (lo, hi) in _spans(c['segments'])) {
          await seedSegment('room-a', lo, hi);
        }
        if (c['storedSeq'] case final int seq) {
          await seedEpoch('e1');
          await seedRow('room-a', seq, connectedAt - 100000);
        }
        final rows = [
          for (final row in (c['walkRows'] ?? const []) as List)
            _roomEvent(row['id'] as String, 'room-a', row['created_at'] as int),
        ];
        // Read before the walk records its own span.
        final resume = await store.resumePoints(lease, const [
          'room-a',
        ], connectedAt);
        final heads = await store.coverageHeads(lease, const [
          'room-a',
        ], connectedAt);
        final walks = <(List<String>, int)>[];
        final result =
            await runner((filters) async {
              final filter = filters.single;
              walks.add((_strings(filter['#h']), filter['since'] as int));
              return rows;
            }).run(
              UserStreamCatchUpJob(
                mode: (c['mode'] as String?) ?? 'live',
                rooms: const ['room-a'],
                seqs: {'room-a': (c['seq'] as int?) ?? 9},
                epoch: 'e1',
                relayPubkey: _relayKey,
                liveAt: liveAt,
                captured: null,
                coverageBefore: connectedAt,
              ),
            );
        final expected = c['expect'] as Map;
        expect(result.classes['room-a']?.name, expected['class']);
        expect(resume['room-a'], expected['resume']);
        expect(
          walks.isEmpty ? null : walks.single.$2,
          expected['since'],
          reason: '$walks',
        );
        final head = heads['room-a'];
        expect(head == null ? null : [head.$1, head.$2], expected['head']);
        for (final name in ['predictedSkips', 'falseSkip']) {
          if (expected[name] case final int count) {
            expect(result.counts[name] ?? 0, count, reason: name);
          }
        }
      });
    }

    test(
      'the EOSE catch-up resumes from before the connection, not a chunk EOSE',
      () async {
        // B1: the chunk carrying room-a reached EOSE at C, after AUTH OK and
        // before the stream EOSE at L; the offline gap must still be walked.
        const connectedAt = 999990;
        const chunkEose = 999998;
        const liveAt = 1000000;
        await seedSegment('room-a', liveAt - 30000, liveAt - 7200);
        await seedSegment('room-a', chunkEose, chunkEose);
        final q = recordingQuery();
        UserStreamCatchUpJob? seen;
        final s = _Stream(
          catchUp: (job, receivedLive) async {
            seen = job;
            return runner(q.query).run(job);
          },
        );
        s.nowMs = connectedAt * 1000;
        await s.stream.connected(1);
        s.nowMs = liveAt * 1000;
        s
          ..send([
            'EVENT',
            'l-1',
            _part1(rooms: const {'room-a': 5}),
          ])
          ..send(['EOSE', 'l-1']);
        await s.stream.settled;
        expect(seen?.coverageBefore, connectedAt);
        expect(seen?.liveAt, liveAt);
        expect(q.walks.single.$2, liveAt - 7200 - storeResumeLookbackSecs);
      },
    );
  });

  group('epoch read barrier', () {
    test('rows of another epoch never read as stored', () async {
      await seedEpoch('e0');
      await seedRow('room-a', 5, 100);
      expect(await store.roomSeqs(lease, const ['room-a']), {
        'room-a': (5, 100),
      });
      expect(
        await store.roomSeqs(lease, const ['room-a'], epoch: 'e1'),
        isEmpty,
      );
      expect(await store.roomSeqs(lease, const ['room-a'], epoch: 'e0'), {
        'room-a': (5, 100),
      });
    });

    test(
      'a failed write keeps the queued epoch; old rows never class equal',
      () async {
        await seedEpoch('e0');
        await seedRow('room-a', 5, 100);
        await seedSegment('room-a', 999000, 999900);
        // A write fails with part 1's epoch queued behind it.
        store.failNextWriteForTest(lease);
        store
          ..submit(lease, [_roomEvent('x1', 'room-b', 999950)])
          ..queueStreamEpoch(lease, 'e1');
        final result = await runner(recordingQuery().query).run(
          const UserStreamCatchUpJob(
            mode: 'gated',
            rooms: ['room-a'],
            seqs: {'room-a': 5},
            epoch: 'e1',
            relayPubkey: _relayKey,
            liveAt: 1000000,
            captured: null,
          ),
          audit: false,
        );
        expect(result.classes['room-a'], isNot(UserStreamRoomClass.equal));
        await store.flushed();
        expect(await storedEpoch(), 'e1');
        expect(await rows(), isEmpty);
      },
    );
  });

  group('batching', () {
    for (final c in _cases('batching')) {
      test(c['name'], () {
        final plan = userStreamBatches(_ints(c['resume']), c['L'] as int);
        final expected = c['expect'] as Map;
        expect(plan.stale, expected['stale']);
        expect([
          for (final batch in plan.batches)
            {'rooms': batch.rooms, 'since': batch.since},
        ], expected['batches']);
      });
    }
  });

  group('ruleW', () {
    for (final c in _cases('ruleW')) {
      test(c['name'], () async {
        // Rows in every room the steps name, so a purge has rooms to forget.
        store.submit(lease, [
          _roomEvent('ea', 'room-a', 10),
          _roomEvent('eb', 'room-b', 10),
          _roomEvent('ex', 'room-x', 10),
          _roomEvent('ek', 'keep', 10),
        ]);
        // The first membership list: a later one forgets what it drops.
        store.forgetRoomsExcept(store.capture(lease), lease, {
          'room-a',
          'room-b',
          'room-x',
          'keep',
        });
        await store.flushed();
        final s = _Stream(
          queueEpoch: (epoch) => store.queueStreamEpoch(lease, epoch),
          recordCoverage: false,
        );
        int? generation;
        final walks = <String, (int, StoreSeqVouch?, int, List<String>)>{};
        for (final raw in c['steps'] as List) {
          final step = (raw as Map).cast<String, dynamic>();
          switch (step['op']) {
            case 'seedEpoch':
              await seedEpoch(step['epoch'] as String);
            case 'seedRow':
              final row = step['row'] as List;
              await seedRow(
                step['room'] as String,
                row[0] as int,
                row[1] as int,
              );
            case 'part1':
              final gen = step['gen'] as int;
              if (gen != generation) {
                await s.stream.connected(gen);
                generation = gen;
              }
              s.send([
                'EVENT',
                'l-1',
                _part1(
                  epoch: step['epoch'] as String,
                  rooms: _ints(step['rooms']),
                ),
              ]);
            case 'streamEose':
              s.send(['EOSE', 'l-1']);
            case 'closed':
              s.send(['CLOSED', 'l-1', 'error: user-stream closed']);
            case 'catchUpStart':
              final rooms = _strings(step['rooms']);
              walks[step['id'] as String] = (
                step['liveAt'] as int,
                s.stream.hooks.vouch(step['gen'] as int, rooms),
                store.capture(lease)!.sequence,
                rooms,
              );
            case 'catchUpEnd':
              final (liveAt, vouch, captured, rooms) = walks[step['id']]!;
              if (step['completed'] == true) {
                store.extendCoverage(
                  lease,
                  rooms,
                  liveAt - 100,
                  liveAt,
                  captured: captured,
                  seqs: vouch?.seqs,
                  epoch: vouch?.epoch,
                );
                s.stream.hooks.walkEnded(
                  vouch,
                  rooms,
                  liveAt,
                  const StoreWalk(true, []),
                  const {},
                );
              }
            case 'purge':
              final keep = {'room-a', 'room-b', 'room-x', 'keep'}
                ..remove(step['room']);
              expect(
                store.forgetRoomsExcept(store.capture(lease), lease, keep),
                isTrue,
              );
            case 'loss':
              store.noteLossForTest(lease);
            default:
              fail('unknown op ${step['op']}');
          }
        }
        final expected = c['expect'] as Map;
        expect(await rows(), expected['rows']);
        expect(
          s.stream.totals['seqWriteSkippedOrder'],
          expected['seqWriteSkippedOrder'],
        );
      });
    }
  });

  group('clearing', () {
    final before = _fixture['clearingRowsBefore'] as Map;
    Future<void> seedAll() async {
      for (final MapEntry(:key, :value) in before.entries) {
        await seedRow(key as String, value[0] as int, value[1] as int);
      }
    }

    for (final c in _cases('clearing')) {
      if (!_strings(c['platforms']).contains('mobile')) continue;
      test('${c['event']} clears ${c['scope']}', () async {
        store.submit(lease, [
          _roomEvent('ea', 'room-a', 10),
          _roomEvent('eb', 'room-b', 10),
        ]);
        store.forgetRoomsExcept(store.capture(lease), lease, {
          'room-a',
          'room-b',
        });
        await seedEpoch('e1');
        await seedAll();
        switch (c['event']) {
          case 'queueFull' || 'writeFailed' || 'memoryGuard':
            // All three end as `writer.lossy`, cleared by the next write.
            store
              ..noteLossForTest(lease)
              ..submit(lease, [_roomEvent('e2', 'room-b', 20)]);
          case 'purge':
            store.forgetRoomsExcept(store.capture(lease), lease, {'room-b'});
          case 'coverageVersion':
            await direct((db) async {
              await db.rawInsert(
                'INSERT OR REPLACE INTO sync_state(key, value) '
                "VALUES ('coverage_version', '1')",
              );
              await dropStaleCoverage(db);
            });
          case 'signOut' || 'scopeEnd':
            await store.forget(_relay, _account);
            lease = store.lease(_relay, _account)!;
          case 'epochChange':
            store.queueStreamEpoch(lease, 'e2');
        }
        final left = await rows();
        expect(
          left.keys,
          c['scope'] == 'room' ? ['room-b'] : isEmpty,
          reason: '${c['event']}',
        );
      });
    }
  });

  group('newRow', () {
    for (final c in _cases('newRow')) {
      test(c['name'], () {
        final liveAt = c['L'] as int;
        final row = c['row'] as Map;
        expect(
          userStreamNewRow(
            inserted: row['inserted'] as bool,
            receivedLive: row['receivedLive'] == true,
            createdAt: row['created_at'] as int,
            liveAt: liveAt,
            head: _head(_spans(c['segments']), liveAt),
          ),
          c['counts'],
        );
      });
    }
  });

  group('skipVerification', () {
    for (final c in _cases('skipVerification')) {
      test(c['name'], () {
        final result = userStreamSkipVerification([
          for (final room in c['rooms'] as List)
            (room['walked'] as bool, room['newRows'] as int),
        ]);
        expect({
          'verifiedSkips': result.verified,
          'falseSkip': result.falseSkip,
        }, c['expect']);
      });
    }
  });

  group('auditSampling', () {
    for (final c in _cases('auditSampling')) {
      test(c['name'], () {
        final eligible = userStreamAuditEligible(
          _ints(c['skipped']),
          c['L'] as int,
        );
        final expected = c['expect'] as Map;
        expect(eligible, (expected['eligible'] as List).toList()..sort());
        final sample = userStreamAuditSample(eligible, Random(7));
        expect(sample, hasLength(expected['sampleSize']));
        expect(eligible, containsAll(sample));
        expect(sample.toSet(), hasLength(sample.length));
      });
    }
  });

  group('auditVerdicts', () {
    for (final c in _cases('auditVerdicts')) {
      test(c['name'], () {
        final completed = <String, bool>{};
        final newRows = <String, int>{};
        for (final batch in c['batches'] as List) {
          for (final room in _strings(batch['rooms'])) {
            completed[room] = batch['completed'] as bool;
          }
          newRows.addAll(_ints(batch['newRows']));
        }
        final result = userStreamAuditVerdicts(completed, newRows);
        expect({
          'skipAudited': result.audited,
          'skipAuditNew': result.auditNew,
          'skipAuditFailed': result.failed,
        }, c['expect']);
      });
    }
  });

  group('liveMissed', () {
    for (final c in _cases('liveMissed')) {
      test(c['name'], () {
        final liveAt = c['L'] as int;
        final prev = c['prev'] as Map;
        final segments = c['segments'] as Map;
        final walked = (c['walked'] as Map).cast<String, bool>();
        final newRows = <UserStreamNewRow>[
          for (final row in c['rows'] as List)
            if (userStreamNewRow(
              inserted: row['inserted'] as bool,
              receivedLive: row['receivedLive'] == true,
              createdAt: row['created_at'] as int,
              liveAt: liveAt,
              head: _head(_spans(segments[row['room']]), liveAt),
            ))
              (
                room: row['room'] as String,
                createdAt: row['created_at'] as int,
                kind: row['kind'] as int,
                pubkey: row['pubkey'] as String,
              ),
        ];
        final result = userStreamLiveMissed(
          prev: prev['eoseAt'] == null
              ? null
              : UserStreamLivePrev(
                  _strings(prev['liveRooms']).toSet(),
                  prev['eoseAt'] as int,
                  prev['lastFrameAt'] as int,
                ),
          completed: {
            for (final MapEntry(:key, :value) in walked.entries)
              if (value) key,
          },
          newRows: newRows,
          relayPubkey: c['relayPubkey'] as String,
        );
        expect({
          'liveRoomsChecked': result.checked,
          'liveMissed': result.missed,
        }, c['expect']);
      });
    }

    test('the live set and bounds round-trip through sync_state', () {
      const prev = UserStreamLivePrev({'b', 'a'}, 100, 200);
      final back = UserStreamLivePrev.decode(prev.encode())!;
      expect(back.rooms, {'a', 'b'});
      expect([back.eoseAt, back.lastFrameAt], [100, 200]);
      expect(UserStreamLivePrev.decode('nope'), isNull);
    });
  });

  group('midConnectionClosed', () {
    for (final c in _cases('midConnectionClosed')) {
      test(c['name'], () async {
        final carried = _strings(c['carriedRooms']);
        for (final MapEntry(:key, :value) in _ints(c['resume']).entries) {
          await seedSegment(key, value - 100, value);
        }
        final q = recordingQuery();
        final jobs = <UserStreamCatchUpJob>[];
        final s = _Stream(
          recordCoverage: false,
          catchUp: (job, receivedLive) async {
            if (!job.fallback) return null;
            jobs.add(job);
            return runner(q.query).run(job);
          },
        );
        await s.stream.connected(1);
        s
          ..send([
            'EVENT',
            'l-1',
            _part1(
              mode: c['mode'] as String,
              rooms: {for (final room in carried) room: 1},
            ),
          ])
          ..send(['EOSE', 'l-1']);
        s.nowMs = (c['closedAt'] as int) * 1000;
        s.send([
          'CLOSED',
          'l-1',
          (c['reason'] as String?) ?? 'error: user-stream closed',
        ]);
        await s.stream.settled;
        final expected = c['expect'] as Map;
        // Chunks on this connection: none while its socket is closing.
        final chunkRooms = s.stream.closingSocket
            ? <String>[]
            : ({
                ...carried,
                ..._strings(c['chunkExtras']),
              }.difference(s.stream.carriedRooms).toList()..sort());
        expect(chunkRooms, expected['chunkRooms']);
        if (s.stream.closingSocket) {
          // The relay dropped frames: nothing counts as live any more, and
          // listeners already see nothing carried (B2, S3).
          expect(s.stream.carriedRooms, isEmpty);
          for (final room in carried) {
            expect(s.stream.liveAtOf(room), isNull);
          }
        }
        final catchUp = expected['catchUp'] as Map?;
        if (catchUp == null) {
          expect(jobs, isEmpty);
        } else {
          expect(jobs.single.liveAt, c['closedAt']);
          expect([
            for (final walk in q.walks) {'rooms': walk.$1, 'since': walk.$2},
          ], catchUp['batches']);
        }
        if (expected['awaitSocketCloseSecs'] case final int secs) {
          expect(s.timers.single.$1, Duration(seconds: secs));
        }
      });
    }
  });

  group('chunkCatchUp', () {
    for (final c in _cases('chunkCatchUp')) {
      test(c['name'], () async {
        final chunkRooms = _strings(c['chunkRooms']);
        for (final room in chunkRooms) {
          await seedSegment(room, 900, 1000);
        }
        final s = _Stream(recordCoverage: false);
        final snapshotRooms = {
          for (final room in _strings(c['snapshotRooms'])) room: 1,
        };
        await s.stream.connected(1);
        switch (c['stream']) {
          case 'none':
            s.send(['CLOSED', 'l-1', 'unsupported: user-stream off']);
          case 'pending':
            break;
          case final String state:
            final mode = state.startsWith('gated')
                ? 'gated'
                : state == 'shadow'
                ? 'shadow'
                : 'live';
            s
              ..send(['EVENT', 'l-1', _part1(mode: mode, rooms: snapshotRooms)])
              ..send(['EOSE', 'l-1']);
            if (state == 'closedAfterEose') {
              s.send(['CLOSED', 'l-1', 'error: user-stream closed']);
            }
        }
        final q = recordingQuery();
        final coverage = RoomBundleCoverage(
          scope: () => (store: store, lease: lease),
          query: q.query,
          nowSecs: () => 2000,
        )..hooks = s.stream.hooks;
        await coverage.catchUp(
          (store: store, lease: lease),
          chunkRooms,
          2000,
          generation: 1,
        );
        final walked = [for (final walk in q.walks) ...walk.$1]..sort();
        expect(walked, (c['expect'] as Map)['chunkCatchUpRooms']);
      });
    }
  });

  group('membership', () {
    for (final c in _cases('membership')) {
      test(c['name'], () async {
        // The EOSE catch-up completes at once, so the snapshot rooms have
        // their stream span (docs/message-sync.md#catch-up-at-the-stream-eose).
        final s = _Stream(
          catchUp: (job, _) async {
            job.roomsDone?.call(job.rooms);
            return UserStreamCatchUpResult();
          },
        );
        await s.stream.connected(1);
        s.send([
          'EVENT',
          'l-1',
          _part1(
            rooms: {
              for (final room in _strings(_fixture['membershipCarriedBefore']))
                room: 1,
            },
          ),
        ]);
        final beforeEose = c['beforeEose'] == true;
        if (!beforeEose) {
          s.send(['EOSE', 'l-1']);
          await s.stream.settled;
        }
        s.coverage.clear();
        for (final raw in c['steps'] as List) {
          final step = (raw as Map).cast<String, dynamic>();
          switch (step['op']) {
            case '44100' || '44101':
              s.stream.noteMembership(
                added: step['op'] == '44100',
                room: step['room'] as String,
                createdAt: step['created_at'] as int,
                pubkey: _relayKey,
                receivedAt: (step['receivedAt'] as int?) ?? 0,
              );
            case 'frame':
              s.send([
                'EVENT',
                'l-1',
                _roomEvent(
                  step['id'] as String,
                  step['room'] as String,
                  step['created_at'] as int,
                ),
              ]);
          }
        }
        if (beforeEose) {
          s.send(['EOSE', 'l-1']);
          await s.stream.settled;
        }
        final expected = c['expect'] as Map;
        expect(s.stored, expected['stored']);
        expect(s.rendered, expected['rendered']);
        expect(s.stream.carriedRooms.toList()..sort(), expected['carried']);
        if (expected['coverage'] case final Map coverage) {
          for (final MapEntry(:key, :value) in coverage.entries) {
            expect(s.coverage[key], _spans(value), reason: '$key');
          }
        }
      });
    }

    test('a rejoin before the catch-up is done writes no span (mobile, '
        'delta review B1)', () async {
      final s = _Stream(); // Its catch-up never completes.
      await s.stream.connected(1);
      s
        ..send([
          'EVENT',
          'l-1',
          _part1(rooms: {'a': 1}),
        ])
        ..send(['EOSE', 'l-1']);
      await s.stream.settled;
      for (final added in [false, true]) {
        s.stream.noteMembership(
          added: added,
          room: 'a',
          createdAt: 100,
          pubkey: _relayKey,
          receivedAt: 120,
        );
      }
      expect(s.stream.carriedRooms, {'a'});
      expect(s.coverage['a'], isNull);
    });

    test('a notice older than the snapshot is ignored', () async {
      final s = _Stream();
      await s.stream.connected(1);
      s
        ..send([
          'EVENT',
          'l-1',
          _part1(rooms: {'a': 1}, createdAt: 500),
        ])
        ..send(['EOSE', 'l-1']);
      s.stream.noteMembership(
        added: false,
        room: 'a',
        createdAt: 499,
        pubkey: _relayKey,
        receivedAt: 600,
      );
      expect(s.stream.carriedRooms, {'a'});
    });

    test('a notice not signed by the relay is ignored', () async {
      final s = _Stream();
      await s.stream.connected(1);
      s
        ..send([
          'EVENT',
          'l-1',
          _part1(rooms: {'a': 1}),
        ])
        ..send(['EOSE', 'l-1']);
      s.stream.noteMembership(
        added: false,
        room: 'a',
        createdAt: 100,
        pubkey: 'someone',
        receivedAt: 100,
      );
      expect(s.stream.carriedRooms, {'a'});
    });
  });

  group('routing', () {
    final lists = _fixture['routingLists'] as Map;
    for (final c in _cases('routing')) {
      test(c['name'], () {
        final route = userStreamRoute(
          (c['frame'] as Map)['room'] as String,
          memberRooms: _strings(lists['memberRooms']).toSet(),
          archivedRooms: _strings(lists['archivedRooms']).toSet(),
        );
        final open = _strings(lists['openRooms']).toSet();
        final room = (c['frame'] as Map)['room'] as String;
        expect({
          'store': true,
          'openRoom': route != UserStreamRoute.unknown && open.contains(room),
          'allRooms': route == UserStreamRoute.member,
          'listRefresh': route == UserStreamRoute.unknown,
        }, c['expect']);
      });
    }

    test((_fixture['routingBurst'] as Map)['name'] as String, () {
      final burst = _fixture['routingBurst'] as Map;
      final timers = <void Function()>[];
      var refreshes = 0;
      final refresher = UserStreamBurst(
        () => refreshes++,
        timer: (_, callback) {
          timers.add(callback);
          return Timer(const Duration(days: 1), () {});
        },
      );
      for (final frame in burst['frames'] as List) {
        final route = userStreamRoute(
          frame['room'] as String,
          memberRooms: const {},
          archivedRooms: const {},
        );
        if (route == UserStreamRoute.unknown) refresher.note();
      }
      for (final timer in timers) {
        timer();
      }
      expect(refreshes, burst['listRefreshes']);
    });
  });

  group('handoff', () {
    for (final c in _cases('handoff')) {
      test(c['name'], () async {
        final s = _Stream(recordCoverage: false);
        await s.stream.connected(1);
        final rooms = {
          ..._strings(c['memberRooms']),
          ..._strings(c['quietRooms']),
        };
        List<String> chunks() =>
            rooms.difference(s.stream.carriedRooms).toList()..sort();
        final expected = c['expect'] as Map;
        expect(chunks(), expected['beforeEose']);
        if (c['mode'] == null) {
          s.send(['CLOSED', 'l-1', 'restricted: not a channel member']);
        } else {
          s.send([
            'EVENT',
            'l-1',
            _part1(
              mode: c['mode'] as String,
              rooms: {for (final room in _strings(c['snapshotRooms'])) room: 1},
            ),
          ]);
          if (c['closedBeforeEose'] == true) {
            s.send(['CLOSED', 'l-1', 'error: user-stream closed']);
          }
          s.send(['EOSE', 'l-1']);
        }
        expect(chunks(), expected['afterEose']);
      });
    }

    test((_fixture['handoffDedupe'] as Map)['name'] as String, () {
      final dedupe = _fixture['handoffDedupe'] as Map;
      final router = RoomLiveRouter((filter, onEvent) async => () {});
      final routed = <String>[];
      router.listenRoomEvents('r', (event) => routed.add(event.id));
      for (final delivery in dedupe['deliveries'] as List) {
        router.routeBundleEvent(
          NostrEvent.fromJson(
            _roomEvent(delivery['id'] as String, 'r', 10)
              ..['sig'] = ''.padRight(128, '0'),
          ),
          const {'r'},
        );
      }
      expect(routed, dedupe['routed']);
    });
  });
}
