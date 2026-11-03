// The mobile user stream recheck (RESEARCH/MOBILE_USER_STREAM_RECHECK_20261103):
// the counters' "already stored" rule (S1), catch-ups that stop with their
// connection and never overlap (S5), integer fields (S3) and the throttled
// unknown-room list refresh (S6). Rules: docs/message-sync.md#counters,
// docs/message-sync.md#snapshot.
import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:math';

import 'package:flutter_test/flutter_test.dart';
import 'package:sqflite_common_ffi/sqflite_ffi.dart';
import 'package:kuzz/shared/message_store/message_store.dart';
import 'package:kuzz/shared/relay/room_bundle_coverage.dart';
import 'package:kuzz/shared/relay/user_stream.dart';

const _account =
    'aa00000000000000000000000000000000000000000000000000000000000001';
const _relay = 'wss://relay.example';
const _relayKey = 'relay';
const _liveAt = 1000000;

Map<String, dynamic> _part1({
  String mode = 'live',
  Map<String, int> rooms = const {},
}) => {
  'id': 'p1',
  'pubkey': _relayKey,
  'created_at': 50,
  'kind': userStreamKind,
  'tags': const [],
  'content': jsonEncode({
    'v': 1,
    'op': 'snapshot',
    'part': 1,
    'parts': 1,
    'mode': mode,
    'epoch': 'e1',
    'kinds': userStreamFrozenKinds,
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

class _Stream {
  _Stream({this.catchUp, this.storedMark});

  final Future<UserStreamCatchUpResult?> Function(
    UserStreamCatchUpJob job,
    bool Function(String id) receivedLive,
  )?
  catchUp;
  final Future<int?> Function()? storedMark;
  var nowMs = _liveAt * 1000;

  late final stream = UserStream(
    UserStreamHost(
      nextSubId: () => 'l-1',
      send: (_) {},
      isActive: (_) => true,
      store: (_) {},
      closeSocket: (_) {},
      scope: () => 'scope',
      readState: (_) async => const {},
      writeState: (_) {},
      chunkRooms: () => const {},
      nowMs: () => nowMs,
      timer: (_, _) => Timer(const Duration(days: 1), () {}),
      storedMark: storedMark ?? () async => null,
      catchUp: catchUp ?? (_, _) async => null,
    ),
  );

  void send(List<Object?> frame) => stream.onMessage(frame);
}

void main() {
  sqfliteFfiInit();
  late Directory dir;
  late MessageStore store;
  late MessageStoreLease lease;

  setUp(() async {
    dir = await Directory.systemTemp.createTemp('user_stream_recheck');
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

  UserStreamCatchUp runner(
    Future<Object?> Function(List<Map<String, dynamic>>) query,
  ) => UserStreamCatchUp(
    scope: (store: store, lease: lease),
    query: query,
    switched: () => false,
    receivedLive: (_) => false,
    random: Random(1),
  );

  /// room-a covered before this connection with seq 5 under e1 (equal).
  Future<void> coverEqualRoom() async {
    store
      ..queueStreamEpoch(lease, 'e1')
      ..extendCoverage(
        lease,
        ['room-a'],
        _liveAt - 30000,
        _liveAt - 7200,
        seqs: {'room-a': 5},
        epoch: 'e1',
      );
    await store.flushed();
  }

  group('stored mark', () {
    test(
      'rows stored after the mark do not count as stored before it',
      () async {
        store.submit(lease, [_roomEvent('before', 'room-a', 10)]);
        final mark = store.storedMark(lease);
        store.submit(lease, [_roomEvent('after', 'room-a', 20)]);
        await store.flushed();
        expect(await store.storedEventIds(lease, ['before', 'after']), {
          'before',
          'after',
        });
        expect(
          await store.storedEventIds(lease, [
            'before',
            'after',
          ], storedBefore: await mark),
          {'before'},
        );
      },
    );

    test('an empty file has mark 0', () async {
      expect(await store.storedMark(lease), 0);
    });
  });

  group('S1: rows stored on this connection are not "already stored"', () {
    // A gap row in an equal room, from the previous live connection's
    // window: a real miss (falseSkip and liveMissed).
    Future<Map<String, int>> run({required bool storedBeforeStart}) async {
      await coverEqualRoom();
      const gapAt = _liveAt - 3600;
      if (storedBeforeStart) {
        store.submit(lease, [_roomEvent('gap', 'room-a', gapAt)]);
      }
      // The connection starts.
      final mark = store.storedMark(lease);
      if (!storedBeforeStart) {
        // A chunk catch-up, the unread refresh or a room reload stores the
        // gap before the stream walk reads it.
        store.submit(lease, [_roomEvent('gap', 'room-a', gapAt)]);
      }
      await store.flushed();
      final result =
          await runner((_) async => [_roomEvent('gap', 'room-a', gapAt)]).run(
            UserStreamCatchUpJob(
              mode: 'live',
              rooms: const ['room-a'],
              seqs: const {'room-a': 5},
              epoch: 'e1',
              relayPubkey: _relayKey,
              liveAt: _liveAt,
              captured: null,
              coverageBefore: _liveAt - 10,
              prev: UserStreamLivePrev({'room-a'}, _liveAt - 30000, _liveAt),
              storedMark: mark,
            ),
            audit: false,
          );
      return result.counts;
    }

    test('stored by another path after the start: counted', () async {
      expect(await run(storedBeforeStart: false), {
        'predictedSkips': 1,
        'falseSkip': 1,
        'liveRoomsChecked': 1,
        'liveMissed': 1,
      });
    });

    test('stored before the start: already stored', () async {
      expect(await run(storedBeforeStart: true), {
        'predictedSkips': 1,
        'verifiedSkips': 1,
        'liveRoomsChecked': 1,
      });
    });

    test(
      'shadow: a chunk catch-up after the EOSE verifies against the mark',
      () async {
        await coverEqualRoom();
        final s = _Stream(
          storedMark: () => store.storedMark(lease),
          catchUp: (job, _) =>
              runner((_) async => const []).run(job, audit: false),
        );
        final coverage = RoomBundleCoverage(
          scope: () => (store: store, lease: lease),
          query: (_) async => [_roomEvent('gap', 'room-a', _liveAt - 3600)],
          nowSecs: () => _liveAt,
          hooks: s.stream.hooks,
        );
        await s.stream.connected(1);
        // The unread refresh stores the gap on this connection.
        store.submit(lease, [_roomEvent('gap', 'room-a', _liveAt - 3600)]);
        s
          ..send([
            'EVENT',
            'l-1',
            _part1(mode: 'shadow', rooms: const {'room-a': 5}),
          ])
          ..send(['EOSE', 'l-1']);
        await s.stream.settled;
        await coverage.catchUp(
          (store: store, lease: lease),
          const ['room-a'],
          _liveAt,
          generation: 1,
        );
        expect((jsonDecode(s.stream.totals.encode()) as Map)..remove('seq'), {
          'predictedSkips': 1,
          'falseSkip': 1,
        });
      },
    );
  });

  group('S5: catch-ups and their connection', () {
    test('a catch-up stops when its connection ends, and the next '
        "connection's waits for it", () async {
      // 200 covered rooms: two batches of 128 and 72.
      final rooms = [for (var i = 0; i < 200; i++) 'r${'$i'.padLeft(3, '0')}'];
      store.extendCoverage(lease, rooms, _liveAt - 30000, _liveAt - 7200);
      await store.flushed();
      final gate = Completer<void>();
      final pages = <String>[];
      var connection = 1;
      final s = _Stream(
        catchUp: (job, _) {
          final owner = connection;
          return runner((filters) async {
            pages.add('$owner:${(filters.single['#h'] as List).length}');
            if (pages.length == 1) await gate.future;
            return const [];
          }).run(job, audit: false);
        },
      );
      await s.stream.connected(1);
      s
        ..send([
          'EVENT',
          'l-1',
          _part1(rooms: {for (final r in rooms) r: 1}),
        ])
        ..send(['EOSE', 'l-1']);
      final first = s.stream.settled!;
      for (var i = 0; i < 50 && pages.isEmpty; i++) {
        await Future<void>.delayed(const Duration(milliseconds: 10));
      }
      expect(pages, ['1:128']);
      s.stream.socketEnded(1);
      connection = 2;
      await s.stream.connected(2);
      s
        ..send([
          'EVENT',
          'l-1',
          _part1(rooms: {for (final r in rooms) r: 1}),
        ])
        ..send(['EOSE', 'l-1']);
      await Future<void>.delayed(const Duration(milliseconds: 50));
      // Connection 2's walk waits for connection 1's.
      expect(pages, ['1:128']);
      gate.complete();
      await first;
      await s.stream.settled;
      // Connection 1 started no second batch after its socket ended.
      expect(pages, ['1:128', '2:128', '2:72']);
    });
  });

  group('S3: integer fields refuse 1.0 and 1e0', () {
    test('a later part numbered 2.0 is refused', () {
      final assembler = UserStreamAssembler();
      final first = jsonDecode(_part1(rooms: const {'a': 1})['content']);
      first['parts'] = 2;
      expect(
        assembler.feed({'kind': userStreamKind, 'content': jsonEncode(first)}),
        isTrue,
      );
      expect(
        assembler.feed({
          'kind': userStreamKind,
          'content': '{"part":2.0,"rooms":{"b":1}}',
        }),
        isFalse,
      );
      expect(assembler.finish(), isNull);
    });
  });

  group('S6: unknown-room list refreshes', () {
    late int nowMs;
    late List<(Duration, void Function())> timers;
    late int refreshes;
    late UserStreamBurst burst;

    setUp(() {
      nowMs = 0;
      timers = [];
      refreshes = 0;
      burst = UserStreamBurst(
        () => refreshes++,
        nowMs: () => nowMs,
        timer: (duration, callback) {
          timers.add((duration, callback));
          return Timer(const Duration(days: 1), () {});
        },
      );
    });

    void fire() {
      final (duration, callback) = timers.removeAt(0);
      nowMs += duration.inMilliseconds;
      callback();
    }

    test('a room refreshes at most once per 10 s', () {
      for (var i = 0; i < 20; i++) {
        burst.note('x');
        nowMs += 400; // 8 s of steady traffic
        if (timers.isNotEmpty && i == 2) fire();
      }
      expect(refreshes, 1);
      expect(timers, isEmpty);
      nowMs = 10000 + 1;
      burst.note('x');
      fire();
      expect(refreshes, 2);
    });

    test('refreshes are at least 5 s apart over all rooms', () {
      burst.note('x');
      expect(timers.single.$1, const Duration(seconds: 1));
      fire();
      expect(refreshes, 1);
      burst.note('y');
      // 1 s after the first refresh: it waits out the 5 s gap.
      expect(timers.single.$1, const Duration(seconds: 5));
      burst.note('z'); // shares the pending refresh
      fire();
      expect(refreshes, 2);
      expect(timers, isEmpty);
    });
  });
}
