// Shared rules with desktop (test-fixtures/user-stream.json,
// docs/message-sync.md#user-stream).
import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/features/channels/room_live_router.dart';
import 'package:kuzz/shared/relay/user_stream.dart';

final Map<String, dynamic> _fixture =
    jsonDecode(File('../test-fixtures/user-stream.json').readAsStringSync())
        as Map<String, dynamic>;

List<Map<String, dynamic>> _cases(String section) => [
  for (final c in _fixture[section] as List) (c as Map).cast<String, dynamic>(),
];

/// A host recording what the stream sends and stores.
class _Host {
  final sent = <List<Object?>>[];
  final stored = <String>[];
  final state = <String, String>{};
  final timers = <(Duration, void Function())>[];
  var nowMs = 1000000;
  var active = true;
  var closedSockets = 0;
  Set<String> chunkRooms = {};

  UserStreamHost get host => UserStreamHost(
    nextSubId: () => 'l-1',
    send: sent.add,
    isActive: (_) => active,
    store: (event) => stored.add(event['id'] as String),
    closeSocket: (_) => closedSockets++,
    scope: () => 'scope',
    readState: (keys) async => {for (final key in keys) key: ?state[key]},
    writeState: state.addAll,
    chunkRooms: () => chunkRooms,
    nowMs: () => nowMs,
    timer: (duration, callback) {
      timers.add((duration, callback));
      return Timer(const Duration(days: 1), () {});
    },
  );
}

String _part1({
  String mode = 'live',
  int parts = 1,
  bool badKinds = false,
  Map<String, int> rooms = const {},
}) => jsonEncode({
  'v': 1,
  'op': 'snapshot',
  'part': 1,
  'parts': parts,
  'mode': mode,
  'epoch': 'e1',
  'kinds': badKinds
      ? userStreamFrozenKinds.skip(1).toList()
      : userStreamFrozenKinds,
  'rooms': rooms,
});

Map<String, dynamic> _frame(String id, int kind, String content) => {
  'id': id,
  'pubkey': 'relay',
  'created_at': 100,
  'kind': kind,
  'tags': const [],
  'content': content,
  'sig': '',
};

void main() {
  tearDown(() => debugUserStreamBuild = null);

  test('frozen kinds are pinned and equal the room live kinds', () {
    expect(userStreamFrozenKinds, _fixture['frozenKinds']);
    expect(userStreamFrozenKinds.toSet(), roomLiveEventKinds.toSet());
    expect(userStreamFilter(), _fixture['request']);
    expect(userStreamKind, _fixture['streamKind']);
    final limits = _fixture['limits'] as Map;
    expect(userStreamBusyRetryDelaysSecs, limits['busyRetryDelaysSecs']);
    expect(userStreamBackoffSecs, limits['backoffSecs']);
    expect(userStreamFullCloseWindowSecs, limits['fullCloseWindowSecs']);
    expect(userStreamSpanExtendSecs, limits['spanExtendSecs']);
    expect(userStreamCompareWindowMs, limits['compareWindowMs']);
    expect(
      userStreamReportIntervalMs,
      (limits['reportIntervalSecs'] as int) * 1000,
    );
    expect(userStreamReportChangeRatio, limits['reportChangeRatio']);
  });

  test('the build switch', () {
    expect(parseUserStreamBuild(''), UserStreamBuild.follow);
    expect(parseUserStreamBuild('follow'), UserStreamBuild.follow);
    expect(parseUserStreamBuild('off'), UserStreamBuild.off);
    expect(() => parseUserStreamBuild('on'), throwsArgumentError);
  });

  group('part1', () {
    for (final c in _cases('part1')) {
      test(c['name'], () {
        final assembler = UserStreamAssembler();
        for (final event in c['events'] as List) {
          assembler.feed((event as Map).cast<String, dynamic>());
        }
        final snapshot = c['eose'] == true ? assembler.finish() : null;
        final expected = c['expect'] as Map;
        expect(snapshot != null, expected['accept']);
        if (snapshot == null) return;
        expect(snapshot.mode.name, expected['mode']);
        expect(snapshot.epoch, expected['epoch']);
        expect(snapshot.rooms, expected['rooms']);
        expect(assembler.ignoredBeforePart1, expected['ignoredBeforePart1']);
      });
    }

    test('seq above 2^53 - 1 and a long epoch are refused', () {
      UserStreamSnapshot? run(Map<String, Object?> content) {
        final assembler = UserStreamAssembler()
          ..feed(_frame('p', userStreamKind, jsonEncode(content)));
        return assembler.finish();
      }

      final good = jsonDecode(_part1()) as Map<String, Object?>;
      expect(
        run({
          ...good,
          'rooms': {'a': userStreamMaxSeq},
        }),
        isNotNull,
      );
      expect(
        run({
          ...good,
          'rooms': {'a': userStreamMaxSeq + 1},
        }),
        isNull,
      );
      expect(run({...good, 'epoch': 'e' * 64}), isNotNull);
      expect(run({...good, 'epoch': 'e' * 65}), isNull);
    });
  });

  group('storeSkip', () {
    for (final c in _cases('storeSkip')) {
      test(c['name'], () async {
        final h = _Host();
        final stream = UserStream(h.host);
        await stream.connected(1);
        expect(h.sent.single, ['REQ', 'l-1', userStreamFilter()]);
        for (final raw in c['frames'] as List) {
          final frame = (raw as Map).cast<String, dynamic>();
          if (frame['eose'] == true) {
            stream.onMessage(['EOSE', 'l-1']);
          } else if (frame['closed'] case final String reason) {
            stream.onMessage(['CLOSED', 'l-1', reason]);
          } else if (frame['close'] == true) {
            stream.debugClientClose();
          } else if (frame['req'] == true) {
            stream.debugResendReq();
          } else {
            final kind = frame['kind'] as int;
            final content = kind != userStreamKind
                ? 'hi'
                : frame.containsKey('mode')
                ? _part1(
                    mode: frame['mode'] as String,
                    parts: frame['parts'] as int? ?? 1,
                    badKinds: frame['badKinds'] == true,
                  )
                : jsonEncode({'rooms': <String, int>{}});
            expect(
              stream.onMessage([
                'EVENT',
                'l-1',
                _frame(frame['id'], kind, content),
              ]),
              isTrue,
            );
          }
        }
        expect(h.stored, c['stored']);
      });
    }
  });

  group('closedReasons', () {
    for (final c in _cases('closedReasons')) {
      test('${c['value']}', () {
        expect(userStreamClosedKey(c['value'] as String), c['key']);
        if (c['match'] == 'prefix') {
          expect(userStreamClosedKey('${c['value']} detail'), c['key']);
        } else {
          expect(userStreamClosedKey('${c['value']} detail'), 'other');
        }
      });
    }
  });

  group('closed', () {
    for (final c in _cases('closed')) {
      test(c['name'], () {
        final verdict = userStreamClosedVerdict(
          c['reason'] as String,
          mode: switch (c['mode']) {
            final String mode => UserStreamMode.values.byName(mode),
            _ => null,
          },
          streamLive: c['streamLive'] as bool,
          afterRetry: c['afterRetry'] == true,
        );
        final expected = c['expect'] as Map;
        expect(verdict.action.name, expected['action']);
        expect(verdict.key, expected['key']);
        expect(verdict.countsTowardBackoff, expected['countsTowardBackoff']);
        expect(verdict.reconnectJitter, expected['reconnectJitter']);
        expect(verdict.bugLog, expected['bugLog']);
      });
    }

    test('a second auth-required on the connection gives chunks', () async {
      final h = _Host();
      final stream = UserStream(h.host);
      await stream.connected(1);
      stream.onMessage(['CLOSED', 'l-1', 'auth-required: authenticate first']);
      expect(h.timers.single.$1, userStreamAuthRetryDelay);
      h.timers.single.$2();
      expect(h.sent.last, ['REQ', 'l-1', userStreamFilter()]);
      expect(stream.phase, UserStreamPhase.pending);
      stream.onMessage(['CLOSED', 'l-1', 'auth-required: authenticate first']);
      expect(stream.phase, UserStreamPhase.none);
      expect(h.timers, hasLength(1));
    });

    test('an overflow in live waits, then closes the socket', () async {
      final h = _Host();
      final stream = UserStream(h.host);
      await stream.connected(1);
      stream
        ..onMessage(['EVENT', 'l-1', _frame('p1', userStreamKind, _part1())])
        ..onMessage(['EOSE', 'l-1'])
        ..onMessage(['CLOSED', 'l-1', 'error: user-stream overflow']);
      expect(stream.fallback, isNull);
      expect(h.timers.single.$1, userStreamAwaitCloseGrace);
      h.timers.single.$2();
      expect(h.closedSockets, 1);
      expect(stream.totals.closed, {'full': 1});
    });

    test('a lagged CLOSED adds 0-10 s to the next reconnect, once', () async {
      final h = _Host();
      final stream = UserStream(h.host);
      await stream.connected(1);
      stream.onMessage(['CLOSED', 'l-1', 'error: user-stream lagged']);
      final jitter = stream.takeReconnectJitterMs();
      expect(jitter, inInclusiveRange(0, 10000));
      expect(stream.takeReconnectJitterMs(), 0);
    });

    test('a fail-closed after a live EOSE asks for the fallback', () async {
      final h = _Host();
      final stream = UserStream(h.host);
      await stream.connected(1);
      stream
        ..onMessage([
          'EVENT',
          'l-1',
          _frame('p1', userStreamKind, _part1(rooms: {'a': 1})),
        ])
        ..onMessage(['EOSE', 'l-1']);
      expect(stream.phase, UserStreamPhase.live);
      stream.onMessage(['CLOSED', 'l-1', 'error: user-stream closed']);
      expect(stream.phase, UserStreamPhase.none);
      expect(stream.fallback?.snapshot.rooms, {'a': 1});
      expect(stream.totals.fallbacks, {'closed': 1});
    });
  });

  group('backoff', () {
    for (final c in _cases('backoff')) {
      test(c['name'], () {
        final events = [
          for (final e in c['events'] as List)
            (e as Map).cast<String, dynamic>(),
        ];
        final checks = [
          for (final e in c['checks'] as List)
            (e as Map).cast<String, dynamic>(),
        ];
        final timeline = [
          for (final e in events) (e['at'] as int, 0, e),
          for (final e in checks) (e['at'] as int, 1, e),
        ]..sort((a, b) => a.$1 != b.$1 ? a.$1 - b.$1 : a.$2 - b.$2);
        var backoff = UserStreamBackoff();
        for (final (at, _, e) in timeline) {
          if (e.containsKey('streamAllowed')) {
            expect(backoff.allowed(at), e['streamAllowed'], reason: 'at $at');
            continue;
          }
          switch (e['op']) {
            case 'closed':
              // The fixture names counting closes by key (full, and closed or
              // other after a live/gated EOSE); see the `closed` cases.
              if (const {'full', 'closed', 'other'}.contains(e['key'])) {
                backoff.noteClose(at);
              }
            case 'networkChange':
              backoff.reset();
            case 'resume':
              backoff.resumed(e['backgroundSecs'] as int);
          }
          // Kept in sync_state: it survives a restart.
          backoff = UserStreamBackoff.decode(backoff.encode());
        }
      });
    }

    test('the stream is not requested while backed off', () async {
      final h = _Host();
      final stream = UserStream(h.host);
      for (var i = 0; i < 2; i++) {
        await stream.connected(i + 1);
        stream
          ..onMessage(['EVENT', 'l-1', _frame('p', userStreamKind, _part1())])
          ..onMessage(['EOSE', 'l-1'])
          ..onMessage(['CLOSED', 'l-1', 'error: user-stream overflow']);
        stream.socketEnded(i + 1);
      }
      h.sent.clear();
      // A restart reads the backoff back.
      final restarted = UserStream(h.host);
      await restarted.connected(3);
      expect(h.sent, isEmpty);
      restarted.networkChanged();
      await restarted.connected(4);
      expect(h.sent.single.first, 'REQ');
    });
  });

  group('lastFrame', () {
    for (final c in _cases('lastFrame')) {
      test(c['name'], () {
        int? last;
        for (final f in c['frames'] as List) {
          last = userStreamLastFrame(
            last,
            (f as Map)['created_at'] as int,
            f['receivedAt'] as int,
          );
        }
        expect(last, c['lastFrameAt']);
      });
    }
  });

  group('comparator', () {
    for (final c in _cases('comparator')) {
      test(c['name'], () {
        final comparator = ShadowComparator(
          snapshotRooms: {...(c['snapshotRooms'] as List).cast<String>()},
          startMs: c['streamEoseAtMs'] as int,
          chunkRooms: () => {...(c['chunkRooms'] as List).cast<String>()},
        );
        for (final j in c['joined'] as List) {
          comparator.joined.add((j as Map)['room'] as String);
        }
        for (final o in c['observations'] as List) {
          final obs = (o as Map).cast<String, dynamic>();
          if (obs['closed'] == true) {
            comparator.stop(obs['atMs'] as int);
            continue;
          }
          comparator.observe(
            onStream: obs['on'] == 'stream',
            id: obs['id'] as String,
            room: obs['room'] as String,
            atMs: obs['atMs'] as int,
          );
        }
        comparator.stop(c['endAtMs'] as int);
        final counts = comparator.takeCounts();
        final expected = c['expect'] as Map;
        for (final MapEntry(:key, :value) in expected.entries) {
          expect(counts[key as String], value, reason: key);
        }
      });
    }
  });

  group('spanStop', () {
    for (final c in _cases('spanStop')) {
      test(c['name'], () {
        final span = UserStreamSpan(c['streamEoseAt'] as int);
        for (final e in c['events'] as List) {
          final event = (e as Map).cast<String, dynamic>();
          if (event['closed'] == true) {
            span.stop();
          } else if (event['on'] == 'stream') {
            span.noteFrame(event['at'] as int);
          }
          // Chunk frames never touch the stream's span.
        }
        expect([span.from, span.to], c['expect']['streamSpan']);
      });
    }
  });

  group('report', () {
    test('logs on a 10% change or after 10 min, and persists', () async {
      final h = _Host();
      final stream = UserStream(h.host);
      await stream.connected(1);
      stream.onMessage(['CLOSED', 'l-1', 'error: user-stream closed']);
      final saved = UserStreamCounters.decode(h.state['user_stream_counters']);
      expect(saved.closed, {'closed': 1});
      expect(saved.seq, 1);
    });

    test('counters decode what they encode', () {
      final counters = UserStreamCounters()
        ..add('eventsCompared', 7)
        ..closed['full'] = 2
        ..snapshotMs = 30
        ..seq = 4;
      final back = UserStreamCounters.decode(counters.encode());
      expect(back.toJson(), counters.toJson());
      expect(back.seq, 4);
    });

    test('the reporter waits for a 10% change or 10 min', () {
      final reporter = UserStreamReporter();
      final counters = UserStreamCounters()..add('eventsCompared', 100);
      expect(reporter.due(counters, 0), isTrue);
      counters.add('eventsCompared', 9);
      expect(reporter.due(counters, 1000), isFalse);
      counters.add('eventsCompared', 1);
      expect(reporter.due(counters, 2000), isTrue);
      counters.add('eventsCompared');
      expect(
        reporter.due(counters, 2000 + userStreamReportIntervalMs - 1),
        isFalse,
      );
      expect(reporter.due(counters, 2000 + userStreamReportIntervalMs), isTrue);
    });
  });

  test('the switch off sends nothing', () async {
    debugUserStreamBuild = UserStreamBuild.off;
    final h = _Host();
    await UserStream(h.host).connected(1);
    expect(h.sent, isEmpty);
  });
}
