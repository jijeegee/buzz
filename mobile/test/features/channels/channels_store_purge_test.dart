import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:sqflite_common_ffi/sqflite_ffi.dart';
import 'package:kuzz/features/channels/channels_provider.dart';
import 'package:kuzz/shared/auth/auth_provider.dart';
import 'package:kuzz/shared/message_store/message_store.dart';
import 'package:kuzz/shared/message_store/message_store_provider.dart';
import 'package:kuzz/shared/read_state/read_state_provider.dart';
import 'package:kuzz/shared/relay/relay.dart';

const _relay = 'https://relay.example';
const _me = 'aa00000000000000000000000000000000000000000000000000000000000001';
const _relayKey =
    'cc00000000000000000000000000000000000000000000000000000000000003';
const _roomA = 'aaaaaaaa-0000-4000-8000-000000000000';
const _roomB = 'bbbbbbbb-0000-4000-8000-000000000000';
const _roomC = 'cccccccc-0000-4000-8000-000000000000';
const _roomN = 'dddddddd-0000-4000-8000-000000000000';

/// The channel list's trigger for docs/message-sync.md#forgetting-a-room:
/// a successful membership fetch forgets the stored rooms of the last list
/// missing from it, keeping rooms shown as joined at settlement and rooms no
/// list had. The first list is only recorded.
void main() {
  sqfliteFfiInit();

  late Directory dir;
  late MessageStore store;
  late _StoreSession session;
  late ProviderContainer container;

  setUp(() async {
    dir = await Directory.systemTemp.createTemp('channels_store_purge');
    store = MessageStore(
      factory: () => databaseFactoryFfi,
      directory: () async => dir.path,
    );
    session = _StoreSession();
    container = ProviderContainer(
      retry: (_, _) => null,
      overrides: [
        authProvider.overrideWith(() => _PendingAuthNotifier()),
        relayConfigProvider.overrideWith(() => _FakeRelayConfigNotifier()),
        relaySessionProvider.overrideWith(() => session),
        messageStoreProvider.overrideWithValue(store),
        myPubkeyProvider.overrideWithValue(_me),
        readStateProvider.overrideWith(_ReadyReadState.new),
        appLifecycleProvider.overrideWith(() => _ResumedLifecycle()),
        relayInfoProvider.overrideWith(
          (ref) async => const RelayInfo(relaySelf: _relayKey),
        ),
      ],
    );
  });

  tearDown(() async {
    container.dispose();
    await store.forget(_relay, _me);
    await dir.delete(recursive: true);
  });

  MessageStoreLease lease() => store.lease(_relay, _me)!;

  test('a fetched list forgets kept rooms missing from it, then trusts the '
      'relay key', () async {
    session.memberships = {_roomA, _roomB};
    container.read(relaySessionProvider);
    store.submit(lease(), [
      _message('a1', _roomA, 100),
      _message('b1', _roomB, 100),
    ]);
    await container.read(channelsProvider.future);
    await _settle();
    await store.flushed();
    expect(await _storedIds(store, lease()), ['a1', 'b1'], reason: 'first');

    // Room B leaves the relay's list; it is still shown joined while that
    // list settles, so the next one forgets it.
    session.memberships = {_roomA};
    for (var i = 0; i < 2; i++) {
      await container.read(channelsProvider.notifier).refresh();
      await _settle();
    }
    await store.flushed();
    expect(await _storedIds(store, lease()), ['a1']);

    // The relay's key was learned with the list, so a leave and rejoin
    // count.
    store.submit(lease(), [
      _notice('l1', _roomA, 250, 'member_left'),
      _notice('j1', _roomA, 300, 'member_joined'),
    ]);
    await store.flushed();
    expect(await _storedIds(store, lease()), ['j1']);
  });

  test('an empty membership answer forgets nothing, whatever is shown '
      'joined', () async {
    session.memberships = {_roomA, _roomB};
    container.read(relaySessionProvider);
    store.submit(lease(), [
      _message('a1', _roomA, 100),
      _message('b1', _roomB, 100),
    ]);
    await container.read(channelsProvider.future);
    await _settle();
    session.memberships = {_roomA};
    await container.read(channelsProvider.notifier).refresh();
    await _settle();

    // Room A alone is shown joined now; a bad answer naming no room must
    // not turn that into the list.
    session.memberships = {};
    await container.read(channelsProvider.notifier).refresh();
    await _settle();
    await store.flushed();
    expect(await _storedIds(store, lease()), ['a1', 'b1']);
  });

  test('a room shown joined at settlement is kept, and one no list had is '
      'left alone', () async {
    session.memberships = {_roomA, _roomC};
    container.read(relaySessionProvider);
    await container.read(channelsProvider.future);
    store.submit(lease(), [
      _message('a1', _roomA, 100),
      _message('b1', _roomB, 100),
      _message('c1', _roomC, 100),
    ]);
    await store.flushed();

    // The relay's list lags behind: room C is shown joined. Room B (browsed)
    // and room N (first rows while the list is in flight) no list had.
    session.memberships = {_roomA};
    final hold = Completer<void>();
    session.holdMemberships = hold;
    final refresh = container.read(channelsProvider.notifier).refresh();
    await _settle();
    store.submit(lease(), [_message('n1', _roomN, 200)]);
    hold.complete();
    await refresh;
    await _settle();
    await store.flushed();
    expect(await _storedIds(store, lease()), ['a1', 'b1', 'c1', 'n1']);
  });
}

Future<void> _settle() async {
  for (var i = 0; i < 20; i++) {
    await Future<void>.delayed(Duration.zero);
  }
}

Map<String, dynamic> _message(String id, String room, int createdAt) => {
  'id': id.padRight(64, '0'),
  'pubkey': 'dd${'0' * 61}4',
  'created_at': createdAt,
  'kind': 9,
  'tags': [
    ['h', room],
  ],
  'content': 'hello',
  'sig': ''.padRight(128, '0'),
};

Map<String, dynamic> _notice(
  String id,
  String room,
  int createdAt,
  String type,
) => {
  ..._message(id, room, createdAt),
  'kind': 40099,
  'pubkey': _relayKey,
  'content': jsonEncode({'type': type, 'actor': _me, 'target': _me}),
};

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

NostrEvent _event(int kind, List<List<String>> tags, String id) => NostrEvent(
  id: id,
  pubkey: 'relay',
  createdAt: 1,
  kind: kind,
  tags: tags,
  content: '',
  sig: 'sig',
);

/// The real session (its store scope included) with canned relay reads.
class _StoreSession extends RelaySessionNotifier {
  Set<String> memberships = {};

  Completer<void>? holdMemberships;

  @override
  SessionState build() {
    super.build();
    return const SessionState(status: SessionStatus.connected);
  }

  List<NostrEvent> _rosters(String? pubkey) => [
    for (final room in memberships)
      _event(39002, [
        ['d', room],
        ['p', pubkey ?? ''],
      ], 'roster-$room'),
  ];

  @override
  Future<List<NostrEvent>> fetchHistory(
    NostrFilter filter, {
    Duration timeout = const Duration(seconds: 8),
  }) async {
    final ids = filter.tags['#d'];
    if (filter.kinds.contains(39000) && ids != null) {
      return [
        for (final room in ids)
          _event(39000, [
            ['d', room],
            ['name', room],
            ['t', 'stream'],
            ['public'],
          ], 'meta-$room'),
      ];
    }
    if (filter.kinds.contains(39002) && ids != null) {
      return _rosters(
        _me,
      ).where((e) => ids.contains(e.getTagValue('d'))).toList();
    }
    return const [];
  }

  @override
  Future<List<NostrEvent>> queryRelay(
    List<NostrFilter> filters, {
    Duration timeout = const Duration(seconds: 8),
  }) async {
    if (filters case [final filter]
        when filter.kinds.length == 1 &&
            filter.kinds.single == 39002 &&
            filter.tags['#p'] != null) {
      final hold = holdMemberships;
      if (hold != null) {
        holdMemberships = null;
        await hold.future;
      }
      if (filter.until != null) return const [];
      final pubkey = filter.tags['#p']!.single;
      return _rosters(
        pubkey,
      ).where((e) => e.tags.any((t) => t[0] == 'p' && t[1] == pubkey)).toList();
    }
    return const [];
  }

  @override
  Future<void Function()> subscribe(
    NostrFilter filter,
    void Function(NostrEvent) onEvent, {
    void Function(String message)? onClosed,
  }) async => () {};
}

class _FakeRelayConfigNotifier extends RelayConfigNotifier {
  @override
  RelayConfig build() => const RelayConfig(baseUrl: _relay, nsec: null);
}

class _PendingAuthNotifier extends AuthNotifier {
  @override
  Future<AuthState> build() => Completer<AuthState>().future;
}

class _ReadyReadState extends ReadStateNotifier {
  @override
  ReadStateState build() => const ReadStateState(
    isReady: true,
    pubkey: _me,
    contexts: {},
    version: 1,
  );
}

class _ResumedLifecycle extends AppLifecycleNotifier {
  @override
  AppLifecycleState build() => AppLifecycleState.resumed;
}
