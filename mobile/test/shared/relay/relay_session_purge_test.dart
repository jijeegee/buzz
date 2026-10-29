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

const _relay = 'https://relay.example';
const _account =
    'aa00000000000000000000000000000000000000000000000000000000000001';
const _relayKey =
    'cc00000000000000000000000000000000000000000000000000000000000003';
const _room = '11111111-1111-4111-8111-111111111111';

Map<String, dynamic> _event(
  String id,
  int createdAt, {
  int kind = 9,
  String pubkey =
      'dd00000000000000000000000000000000000000000000000000000000000004',
  String content = 'hello',
}) => {
  'id': id.padRight(64, '0'),
  'pubkey': pubkey,
  'created_at': createdAt,
  'kind': kind,
  'tags': [
    ['h', _room],
  ],
  'content': content,
  'sig': ''.padRight(128, '0'),
};

Map<String, dynamic> _notice(String id, int createdAt, String type) => _event(
  id,
  createdAt,
  kind: 40099,
  pubkey: _relayKey,
  content: jsonEncode({'type': type, 'actor': _account, 'target': _account}),
);

Map<String, dynamic> _joined(String id, int createdAt) =>
    _notice(id, createdAt, 'member_joined');

Map<String, dynamic> _left(String id, int createdAt) =>
    _notice(id, createdAt, 'member_left');

/// The session's part in docs/message-sync.md#forgetting-a-room: a
/// relay-signed leave and rejoin forgets the room whichever subscription brings it, and
/// a membership list or relay key only applies under the current scope.
void main() {
  sqfliteFfiInit();

  late Directory dir;
  late MessageStore store;
  late RelaySessionNotifier session;
  late _RecordingRelaySocket socket;

  setUp(() async {
    dir = await Directory.systemTemp.createTemp('relay_session_purge');
    store = MessageStore(
      factory: () => databaseFactoryFfi,
      directory: () async => dir.path,
    );
    session = RelaySessionNotifier(
      httpClient: http_testing.MockClient(
        (_) async => http.Response('[]', 200),
      ),
    );
    final container = ProviderContainer(
      overrides: [
        authProvider.overrideWith(() => _PendingAuthNotifier()),
        relaySessionProvider.overrideWith(() => session),
        relayConfigProvider.overrideWith(() => _FakeRelayConfigNotifier()),
        myPubkeyProvider.overrideWithValue(_account),
        messageStoreProvider.overrideWithValue(store),
      ],
    );
    addTearDown(() async {
      container.dispose();
      await store.forget(_relay, _account);
      await dir.delete(recursive: true);
    });
    container.read(relaySessionProvider);
    socket = _RecordingRelaySocket();
    session.debugAttachSocketForTest(socket);
  });

  Future<String> subscribeRoom() async {
    unawaited(
      session.subscribe(
        const NostrFilter(
          kinds: EventKind.channelEventKinds,
          tags: {
            '#h': [_room],
          },
          limit: 0,
        ),
        (_) {},
      ),
    );
    await Future<void>.delayed(Duration.zero);
    return socket.messages.lastWhere((message) => message.first == 'REQ')[1]
        as String;
  }

  test('a relay rejoin forgets the room before EOSE too', () async {
    final scope = session.storeScope!;
    store.submit(scope.lease, [_event('a1', 100)]);
    session.noteStoreRelayKey(scope, _relayKey);
    final subId = await subscribeRoom();

    session.debugHandleMessage(['EVENT', subId, _joined('jf', 200)]);
    await store.flushed();
    expect(await _storedIds(store, scope.lease), [
      'a1',
      'jf',
    ], reason: 'first join');

    session.debugHandleMessage(['EVENT', subId, _left('l1', 250)]);
    session.debugHandleMessage(['EVENT', subId, _joined('j1', 300)]);
    await store.flushed();
    expect(await _storedIds(store, scope.lease), ['j1']);
  });

  test('a relay key noted under a stale scope is not trusted', () async {
    final scope = session.storeScope!;
    store.submit(scope.lease, [_event('a1', 100)]);
    // Another account on this server: the scope is no longer current.
    final stale = store.capture(store.lease(_relay, _relayKey));
    session.noteStoreRelayKey(stale, _relayKey);
    final subId = await subscribeRoom();

    session.debugHandleMessage(['EVENT', subId, _left('l1', 250)]);
    session.debugHandleMessage(['EVENT', subId, _joined('j1', 300)]);
    await store.flushed();
    expect(await _storedIds(store, scope.lease), ['a1', 'j1', 'l1']);
  });

  test('only a list under the current scope is applied', () async {
    final otherServer = store.capture(
      store.lease('https://other.example', _account),
    );
    final otherAccount = store.capture(store.lease(_relay, _relayKey));
    expect(otherServer, isNotNull);
    expect(otherAccount, isNotNull);
    expect(session.forgetStoredRoomsExcept(otherServer, {'x'}), isFalse);
    expect(session.forgetStoredRoomsExcept(otherAccount, {'x'}), isFalse);
    expect(session.forgetStoredRoomsExcept(session.storeScope, {}), isFalse);
    expect(session.forgetStoredRoomsExcept(session.storeScope, {'x'}), isTrue);
  });
}

Future<List<String>> _storedIds(
  MessageStore store,
  MessageStoreLease lease,
) async {
  // A second connection: closing it must not close the store's own.
  final db = await databaseFactoryFfi.openDatabase(
    await store.pathOf(lease),
    options: OpenDatabaseOptions(singleInstance: false),
  );
  try {
    final rows = await db.rawQuery('SELECT id FROM events ORDER BY id');
    return [
      for (final row in rows)
        (row['id']! as String).replaceFirst(RegExp(r'0+$'), ''),
    ];
  } finally {
    await db.close();
  }
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
