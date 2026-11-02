import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart' as http_testing;
import 'package:sqflite_common_ffi/sqflite_ffi.dart';
import 'package:kuzz/shared/auth/auth_provider.dart';
import 'package:kuzz/shared/message_store/message_store.dart';
import 'package:kuzz/shared/message_store/message_store_provider.dart';
import 'package:kuzz/shared/relay/relay.dart';
import 'package:kuzz/shared/relay/user_stream.dart';

const _relay = 'https://relay.example';
const _account =
    'aa00000000000000000000000000000000000000000000000000000000000001';
const _other =
    'bb00000000000000000000000000000000000000000000000000000000000002';
const _relayKey =
    'cc00000000000000000000000000000000000000000000000000000000000003';
const _room = '11111111-1111-4111-8111-111111111111';

Map<String, dynamic> _event(String id, {int kind = 9, String? content}) => {
  'id': id.padRight(64, '0'),
  'pubkey': _relayKey,
  'created_at': 100,
  'kind': kind,
  'tags': [
    ['h', _room],
  ],
  'content': content ?? 'hello',
  'sig': ''.padRight(128, '0'),
};

Map<String, dynamic> _part1(String mode) => _event(
  'p1',
  kind: userStreamKind,
  content: jsonEncode({
    'v': 1,
    'op': 'snapshot',
    'part': 1,
    'parts': 1,
    'mode': mode,
    'epoch': 'e1',
    'kinds': userStreamFrozenKinds,
    'rooms': {_room: 3},
  }),
);

/// The session's part of docs/message-sync.md#user-stream: the stream REQ
/// goes first after AUTH OK, the store skip runs before `submit`, the
/// stream never enters the generic live map, and history REQs retry
/// `error: busy` on fresh sub ids.
void main() {
  sqfliteFfiInit();

  late Directory dir;
  late MessageStore store;
  late RelaySessionNotifier session;
  late _RecordingRelaySocket socket;
  late List<(Duration, void Function())> timers;

  Future<void> start(String account) async {
    dir = await Directory.systemTemp.createTemp('relay_session_user_stream');
    store = MessageStore(
      factory: () => databaseFactoryFfi,
      directory: () async => dir.path,
    );
    timers = [];
    session = RelaySessionNotifier(
      httpClient: http_testing.MockClient(
        (_) async => http.Response('[]', 200),
      ),
      retryTimerFactory: (duration, callback) {
        timers.add((duration, callback));
        return Timer(const Duration(days: 1), () {});
      },
    );
    final container = ProviderContainer(
      overrides: [
        authProvider.overrideWith(() => _PendingAuthNotifier()),
        relaySessionProvider.overrideWith(() => session),
        relayConfigProvider.overrideWith(() => _FakeRelayConfigNotifier()),
        myPubkeyProvider.overrideWithValue(account),
        messageStoreProvider.overrideWithValue(store),
      ],
    );
    addTearDown(() async {
      container.dispose();
      await store.forget(_relay, account);
      await dir.delete(recursive: true);
    });
    container.read(relaySessionProvider);
    socket = _RecordingRelaySocket();
    session.debugAttachSocketForTest(socket);
  }

  Future<String> connect() async {
    await session.debugHandleConnected();
    for (var i = 0; i < 20 && socket.messages.isEmpty; i++) {
      await Future<void>.delayed(const Duration(milliseconds: 5));
    }
    final req = socket.messages.firstWhere(
      (m) => m.first == 'REQ' && isUserStreamFilter(m[2]),
    );
    return req[1] as String;
  }

  Future<List<String>> storedIds() async {
    await store.flushed();
    final lease = session.storeScope!.lease;
    final path = await store.pathOf(lease);
    if (!File(path).existsSync()) return [];
    final db = await databaseFactoryFfi.openDatabase(
      path,
      options: OpenDatabaseOptions(singleInstance: false),
    );
    try {
      final rows = await db.rawQuery('SELECT id FROM events ORDER BY id');
      return [for (final row in rows) (row['id'] as String).substring(0, 2)];
    } finally {
      await db.close();
    }
  }

  test(
    'the stream REQ goes out after AUTH OK with the frozen filter',
    () async {
      await start(_account);
      final subId = await connect();
      final req = socket.messages.firstWhere((m) => m[1] == subId);
      expect(req, ['REQ', subId, userStreamFilter()]);
      expect(subId, startsWith('l-'));

      // Never replayed as a generic live subscription.
      socket.messages.clear();
      await session.debugReplayLiveSubscriptions();
      expect(
        socket.messages.where(
          (m) => m.first == 'REQ' && isUserStreamFilter(m[2]),
        ),
        isEmpty,
      );
    },
  );

  test(
    'live: only frames after an accepted EOSE are stored; 24220 never',
    () async {
      await start(_account);
      final subId = await connect();
      session
        ..debugHandleMessage(['EVENT', subId, _event('x0')])
        ..debugHandleMessage(['EVENT', subId, _part1('live')])
        ..debugHandleMessage(['EVENT', subId, _event('x1')])
        ..debugHandleMessage(['EOSE', subId])
        ..debugHandleMessage(['EVENT', subId, _event('x2')])
        ..debugHandleMessage(['EVENT', subId, _part1('live')])
        ..debugHandleMessage(['CLOSED', subId, 'error: user-stream closed'])
        ..debugHandleMessage(['EVENT', subId, _event('x3')]);
      expect(await storedIds(), ['x2']);
      expect(session.userStream.fallback, isNotNull);
    },
  );

  test('shadow stores nothing from the stream', () async {
    await start(_account);
    final subId = await connect();
    session
      ..debugHandleMessage(['EVENT', subId, _part1('shadow')])
      ..debugHandleMessage(['EOSE', subId])
      ..debugHandleMessage(['EVENT', subId, _event('x1')]);
    expect(await storedIds(), isEmpty);
    expect(session.userStream.phase, UserStreamPhase.shadow);
  });

  test('an old relay CLOSED leaves everything as without the stream', () async {
    await start(_account);
    final subId = await connect();
    session.debugHandleMessage([
      'CLOSED',
      subId,
      'restricted: not a channel member',
    ]);
    expect(session.userStream.phase, UserStreamPhase.none);
    // A late frame on the id is still kept away from the store.
    session.debugHandleMessage(['EVENT', subId, _event('x1')]);
    expect(await storedIds(), isEmpty);
  });

  test('error: busy on a history REQ retries on a fresh sub id', () async {
    await start(_account);
    await session.debugHandleConnected();
    final result = session.fetchHistory(
      const NostrFilter(kinds: [9], limit: 5),
      timeout: const Duration(minutes: 5),
    );
    await Future<void>.delayed(Duration.zero);
    String lastHistoryReq() =>
        socket.messages.lastWhere(
              (m) => m.first == 'REQ' && '${m[1]}'.startsWith('h-'),
            )[1]
            as String;
    final first = lastHistoryReq();
    final ids = <String>{first};
    for (var i = 0; i < 7; i++) {
      final current = lastHistoryReq();
      session.debugHandleMessage(['CLOSED', current, 'error: busy']);
      // Late frames of the refused REQ are not the retry's.
      session.debugHandleMessage(['EVENT', current, _event('la')]);
      expect(
        timers.last.$1,
        Duration(seconds: userStreamBusyRetryDelaysSecs[i]),
      );
      timers.last.$2();
      ids.add(lastHistoryReq());
    }
    expect(ids, hasLength(8));
    final retry = lastHistoryReq();
    session
      ..debugHandleMessage(['EVENT', retry, _event('ok')])
      ..debugHandleMessage(['EOSE', retry]);
    final events = await result;
    expect(events.map((e) => e.id.substring(0, 2)), ['ok']);
  });

  test('other CLOSED reasons still fail a history REQ', () async {
    await start(_account);
    await session.debugHandleConnected();
    final result = session.fetchHistory(const NostrFilter(kinds: [9]));
    await Future<void>.delayed(Duration.zero);
    final id = socket.messages.lastWhere(
      (m) => m.first == 'REQ' && '${m[1]}'.startsWith('h-'),
    )[1];
    session.debugHandleMessage(['CLOSED', id, 'error: shutting down']);
    await expectLater(result, throwsException);
  });

  test('counters are kept per account', () async {
    await start(_account);
    final subId = await connect();
    session.debugHandleMessage(['CLOSED', subId, 'error: user-stream closed']);
    await store.flushed();
    final mine = await store.readSyncState(session.storeScope!.lease, [
      'user_stream_counters',
    ]);
    expect(UserStreamCounters.decode(mine['user_stream_counters']).closed, {
      'closed': 1,
    });
    final theirs = await store.readSyncState(store.lease(_relay, _other), [
      'user_stream_counters',
    ]);
    expect(theirs, isEmpty);
  });

  test('the switch off sends no stream REQ', () async {
    debugUserStreamBuild = UserStreamBuild.off;
    addTearDown(() => debugUserStreamBuild = null);
    await start(_account);
    await session.debugHandleConnected();
    await Future<void>.delayed(const Duration(milliseconds: 20));
    expect(
      socket.messages.where(
        (m) => m.first == 'REQ' && isUserStreamFilter(m[2]),
      ),
      isEmpty,
    );
  });
}

class _FakeRelayConfigNotifier extends RelayConfigNotifier {
  @override
  RelayConfig build() => const RelayConfig(baseUrl: _relay, nsec: null);
}

class _PendingAuthNotifier extends AuthNotifier {
  @override
  Future<AuthState> build() => Completer<AuthState>().future;
}

class _RecordingRelaySocket extends RelaySocket {
  _RecordingRelaySocket()
    : super(
        wsUrl: 'wss://relay.example',
        nsec: null,
        onMessage: (_) {},
        onConnected: () {},
        onDisconnected: (_) {},
      );

  final List<List<dynamic>> messages = [];

  @override
  void send(List<dynamic> payload) => messages.add(payload);

  @override
  void dispose() {}
}
