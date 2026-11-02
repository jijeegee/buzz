// The user stream's per-connection and per-account state
// (docs/message-sync.md#user-stream): what listeners see after an overflow,
// the state load across account switches, accounts in different modes, the
// auth-required fallback, the network-change filter and the iOS report.
import 'dart:async';
import 'dart:convert';

import 'package:connectivity_plus/connectivity_plus.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/shared/relay/app_lifecycle_provider.dart';
import 'package:kuzz/shared/relay/user_stream.dart';

/// A host whose state lives per scope (account and server), with reads that
/// a test can hold back.
class _Host {
  Object? scope = 'A';
  final sent = <List<Object?>>[];
  final states = <Object, Map<String, String>>{};
  final held = <Object, Completer<void>>{};
  final timers = <(Duration, void Function())>[];
  final jobs = <UserStreamCatchUpJob>[];
  final posted = <UserStreamCounters>[];
  var nowMs = 1000000000;
  var closedSockets = 0;
  var subIds = 0;
  String? reportPlatform;

  late final stream = UserStream(
    UserStreamHost(
      nextSubId: () => 'l-${++subIds}',
      send: sent.add,
      isActive: (_) => true,
      store: (_) {},
      closeSocket: (_) => closedSockets++,
      scope: () => scope,
      readState: (keys) async {
        final at = scope!;
        await held[at]?.future;
        final state = states[at] ?? const {};
        return {for (final key in keys) key: ?state[key]};
      },
      writeState: (values) => (states[scope!] ??= {}).addAll(values),
      chunkRooms: () => const {},
      nowMs: () => nowMs,
      timer: (duration, callback) {
        timers.add((duration, callback));
        return Timer(const Duration(days: 1), () {});
      },
      catchUp: (job, _) async {
        jobs.add(job);
        return null;
      },
      reportPlatform: reportPlatform,
      postReport: posted.add,
    ),
  );

  String get subId => 'l-$subIds';

  void snapshot(String mode, Map<String, int> rooms, {String epoch = 'e1'}) {
    stream
      ..onMessage(['EVENT', subId, _part1(mode, rooms, epoch)])
      ..onMessage(['EOSE', subId]);
  }

  UserStreamCounters counters(Object scope) =>
      UserStreamCounters.decode(states[scope]?['user_stream_counters']);
}

Map<String, dynamic> _part1(
  String mode,
  Map<String, int> rooms,
  String epoch,
) => {
  'id': 'p1',
  'pubkey': 'relay',
  'created_at': 100,
  'kind': userStreamKind,
  'tags': const [],
  'content': jsonEncode({
    'v': 1,
    'op': 'snapshot',
    'part': 1,
    'parts': 1,
    'mode': mode,
    'epoch': epoch,
    'kinds': userStreamFrozenKinds,
    'rooms': rooms,
  }),
  'sig': '',
};

void main() {
  group('carried rooms after an overflow (B2, S3)', () {
    test('listeners see nothing carried at the CLOSED, and after a backed-off '
        'reconnect', () async {
      final h = _Host();
      final stream = h.stream;
      // What the room list's glue does: re-read carriedRooms on each notify.
      var glue = <String>{};
      stream.addListener(() => glue = stream.carriedRooms);
      for (var g = 1; g <= 2; g++) {
        await stream.connected(g);
        h.snapshot('live', {'a': 1, 'b': 1});
        expect(glue, {'a', 'b'});
        expect(stream.liveAtOf('a'), isNotNull);
        stream.onMessage(['CLOSED', h.subId, 'error: user-stream overflow']);
        // At once: nothing carried or live, and no chunks on this socket.
        expect(glue, isEmpty);
        expect(stream.closingSocket, isTrue);
        expect(stream.liveAtOf('a'), isNull);
        h.nowMs += 1000;
        stream.socketEnded(g);
        expect(stream.closingSocket, isFalse);
      }
      // Third connection: backed off, so no REQ, and nothing carried.
      h.sent.clear();
      glue = {'stale'};
      await stream.connected(3);
      expect(h.sent, isEmpty, reason: 'the backoff blocks the REQ');
      expect(glue, isEmpty);
    });

    test('lagged: the same, and the socket closes after 2 s', () async {
      final h = _Host();
      var glue = <String>{};
      h.stream.addListener(() => glue = h.stream.carriedRooms);
      await h.stream.connected(1);
      h.snapshot('gated', {'a': 1});
      h.stream.onMessage(['CLOSED', h.subId, 'error: user-stream lagged']);
      expect(glue, isEmpty);
      expect(h.timers.single.$1, userStreamAwaitCloseGrace);
      h.timers.single.$2();
      expect(h.closedSockets, 1);
    });
  });

  group('auth-required after the stream EOSE', () {
    test(
      'live: falls back to chunks with a catch-up from the CLOSED',
      () async {
        final h = _Host();
        await h.stream.connected(1);
        h.snapshot('live', {'a': 1, 'b': 2});
        h.nowMs += 5000;
        h.sent.clear();
        h.stream.onMessage(['CLOSED', h.subId, 'auth-required: authenticate']);
        expect(h.stream.carriedRooms, isEmpty);
        expect(h.stream.phase, UserStreamPhase.none);
        // No REQ again on this connection, no retry timer.
        expect(h.sent, isEmpty);
        expect(h.timers, isEmpty);
        final fallback = h.jobs.last;
        expect(fallback.fallback, isTrue);
        expect(fallback.rooms, ['a', 'b']);
        expect(fallback.liveAt, h.nowMs ~/ 1000);
        expect(h.stream.totals.fallbacks, {'auth_required': 1});
      },
    );

    test('before the EOSE: retried once after a second', () async {
      final h = _Host();
      await h.stream.connected(1);
      h.sent.clear();
      h.stream.onMessage(['CLOSED', h.subId, 'auth-required: authenticate']);
      expect(h.timers.single.$1, userStreamAuthRetryDelay);
      h.timers.single.$2();
      expect(h.sent.single.first, 'REQ');
    });
  });

  group('state per account (S1)', () {
    test('a late read of the previous account changes nothing', () async {
      final h = _Host();
      h.states['B'] = {
        'user_stream_counters':
            (UserStreamCounters()
                  ..add('reconnects', 7)
                  ..seq = 4)
                .encode(),
        'user_stream_backoff': jsonEncode({
          'closes': <int>[],
          'until': h.nowMs ~/ 1000 + 3600,
        }),
      };
      h.states['A'] = {
        'user_stream_counters':
            (UserStreamCounters()
                  ..add('reconnects', 99)
                  ..seq = 50)
                .encode(),
      };
      // Cold start on A, whose read is slow.
      final slowA = h.held['A'] = Completer<void>();
      final first = h.stream.connected(1);
      // Switch to B before A's read lands; B's read is quick.
      h.scope = 'B';
      await h.stream.connected(2);
      expect(h.sent, isEmpty, reason: "B's backoff holds");
      slowA.complete();
      await first;
      expect(h.sent, isEmpty, reason: 'nothing for A on B');
      // B's numbers stand; nothing of A's was written into B's file.
      expect(h.stream.totals['reconnects'], 7);
      expect(h.stream.totals.seq, 4);
      h.stream.socketEnded(2);
      expect(h.counters('B')['reconnects'], 7);
      expect(h.counters('B').seq, 4);
      expect(h.counters('A')['reconnects'], 99);
    });

    test('nothing is written for an account before its state loads', () async {
      final h = _Host();
      h.states['B'] = {
        'user_stream_counters': (UserStreamCounters()..seq = 3).encode(),
      };
      await h.stream.connected(1);
      h.stream.onMessage(['CLOSED', h.subId, 'error: user-stream closed']);
      final heldB = h.held['B'] = Completer<void>();
      h.scope = 'B';
      final pending = h.stream.connected(2);
      // A network change before B loaded applies once it has.
      h.stream.networkChanged();
      expect(h.states['B']!.keys, ['user_stream_counters']);
      heldB.complete();
      await pending;
      // B goes on from its own seq (one more for this reconnect's line),
      // with none of A's closes.
      expect(h.counters('B').seq, 4);
      expect(h.counters('B').closed, isEmpty);
      expect(h.counters('A').closed, {'closed': 1});
    });

    test(
      'two accounts in different modes keep their own mode and counters',
      () async {
        final h = _Host();
        await h.stream.connected(1);
        h.snapshot('live', {'a': 1});
        expect(h.stream.carriedRooms, {'a'});
        h.stream.onMessage(['CLOSED', h.subId, 'error: user-stream closed']);
        h.stream.socketEnded(1);
        h.scope = 'B';
        await h.stream.connected(2);
        h.snapshot('shadow', {'a': 1, 'b': 1});
        // The mode is the connection's: B is in shadow, nothing carried.
        expect(h.stream.phase, UserStreamPhase.shadow);
        expect(h.stream.carriedRooms, isEmpty);
        expect(h.jobs.last.mode, 'shadow');
        h.stream.onMessage(['CLOSED', h.subId, 'error: user-stream overflow']);
        h.stream.socketEnded(2);
        expect(h.counters('A').closed, {'closed': 1});
        expect(h.counters('A').fallbacks, {'closed': 1});
        expect(h.counters('B').closed, {'shadow_full': 1});
        expect(h.counters('B').fallbacks, isEmpty);
        // Back on A: live again, A's counters continue.
        h.scope = 'A';
        await h.stream.connected(3);
        h.snapshot('live', {'a': 1});
        expect(h.stream.carriedRooms, {'a'});
        expect(h.stream.totals.closed, {'closed': 1});
      },
    );
  });

  group('received ids', () {
    test('a connection whose stream is gone keeps no chunk ids', () async {
      final h = _Host();
      await h.stream.connected(1);
      h.stream.noteLiveEvent('x1');
      expect(h.stream.receivedLive('x1'), isTrue);
      h.stream.onMessage([
        'CLOSED',
        h.subId,
        'restricted: not a channel member',
      ]);
      h.stream.noteLiveEvent('x2');
      expect(h.stream.receivedLive('x1'), isFalse);
      expect(h.stream.receivedLive('x2'), isFalse);
    });
  });

  group('network change (S4)', () {
    test('the first event and repeats never count', () {
      final filter = NetworkChangeFilter();
      expect(filter.changed([ConnectivityResult.wifi]), isFalse);
      expect(filter.changed([ConnectivityResult.wifi]), isFalse);
      expect(filter.changed([ConnectivityResult.mobile]), isTrue);
      expect(filter.changed([ConnectivityResult.mobile]), isFalse);
      expect(filter.changed([ConnectivityResult.none]), isFalse);
      expect(filter.changed([ConnectivityResult.mobile]), isTrue);
      expect(
        filter.changed([ConnectivityResult.mobile, ConnectivityResult.vpn]),
        isTrue,
      );
    });
  });

  group('iOS report (S6)', () {
    test('report lines are posted at most every 5 min, iOS only', () async {
      final h = _Host()..reportPlatform = 'ios';
      await h.stream.connected(1);
      h.stream.onMessage(['CLOSED', h.subId, 'error: user-stream closed']);
      expect(h.posted, hasLength(1));
      expect(h.posted.single.closed, {'closed': 1});
      h.stream.socketEnded(1);
      await h.stream.connected(2);
      h.stream.onMessage(['CLOSED', h.subId, 'error: user-stream closed']);
      expect(h.posted, hasLength(1), reason: 'within 5 min');
      h.nowMs += userStreamReportIntervalPostMs;
      h.stream.socketEnded(2);
      await h.stream.connected(3);
      h.stream.onMessage(['CLOSED', h.subId, 'error: user-stream closed']);
      expect(h.posted, hasLength(2));

      final other = _Host();
      await other.stream.connected(1);
      other.stream.onMessage([
        'CLOSED',
        other.subId,
        'error: user-stream closed',
      ]);
      expect(other.posted, isEmpty);
    });

    test('the body has fixed fields, all u32', () {
      final counters = UserStreamCounters()
        ..add('eventsCompared', 1200)
        ..add('chunkOnly', -3)
        ..closed['full'] = 1
        ..closed['made_up'] = 5
        ..seq = 9;
      final body = userStreamReportBody(
        counters,
        platform: 'ios',
        appVersion: '0.6.17',
      );
      expect(body.keys, [
        'platform',
        'app_version',
        'seq',
        'counters',
        'closed',
      ]);
      final fields = body['counters']! as Map;
      expect(fields.keys, [...userStreamCounterNames, 'snapshotMs']);
      expect(fields['eventsCompared'], 1200);
      expect(fields['chunkOnly'], 0);
      final closed = body['closed']! as Map;
      expect(closed.keys, userStreamReportClosedKeys);
      expect(closed['full'], 1);
      expect(utf8.encode(jsonEncode(body)).length, lessThan(4096));
    });
  });
}
