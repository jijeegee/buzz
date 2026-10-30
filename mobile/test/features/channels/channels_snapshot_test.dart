import 'dart:async';
import 'dart:io';

import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:sqflite_common_ffi/sqflite_ffi.dart';
import 'package:kuzz/features/channels/channel.dart';
import 'package:kuzz/features/channels/channel_snapshot_codec.dart';
import 'package:kuzz/features/channels/channels_provider.dart';
import 'package:kuzz/shared/auth/auth_provider.dart';
import 'package:kuzz/shared/message_store/message_store.dart';
import 'package:kuzz/shared/message_store/message_store_provider.dart';
import 'package:kuzz/shared/read_state/read_state_provider.dart';
import 'package:kuzz/shared/relay/relay.dart';

const _relay = 'https://relay.example';
const _me = 'aa00000000000000000000000000000000000000000000000000000000000001';
const _other =
    'bb00000000000000000000000000000000000000000000000000000000000002';
const _relayKey =
    'cc00000000000000000000000000000000000000000000000000000000000003';
const _roomA = 'aaaaaaaa-0000-4000-8000-000000000000';
const _roomB = 'bbbbbbbb-0000-4000-8000-000000000000';

final _account = NotifierProvider<_Account, String>(_Account.new);

class _Account extends Notifier<String> {
  @override
  String build() => _me;

  void switchTo(String pubkey) => state = pubkey;
}

Channel _saved(String id) => Channel(
  id: id,
  name: 'saved-$id',
  channelType: 'stream',
  visibility: 'open',
  description: '',
  createdBy: '',
  createdAt: DateTime.utc(2026),
  memberCount: 1,
  isMember: true,
);

/// The saved channel list (docs/message-sync.md#launch-order): drawn from
/// the account's store file before the relay's list, replaced by it, never
/// counted as a list for the left-room purge, and never shown to another
/// account.
void main() {
  sqfliteFfiInit();

  late Directory dir;
  late MessageStore store;
  late _StoreSession session;
  late ProviderContainer container;

  setUp(() async {
    dir = await Directory.systemTemp.createTemp('channels_snapshot');
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
        myPubkeyProvider.overrideWith((ref) => ref.watch(_account)),
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
    await store.forget(_relay, _other);
    await dir.delete(recursive: true);
  });

  MessageStoreLease lease([String pubkey = _me]) =>
      store.lease(_relay, pubkey)!;

  Future<void> saveList(List<String> ids, [String pubkey = _me]) =>
      store.writeSnapshots(lease(pubkey), {
        channelListSnapshotKey: encodeChannelListSnapshot([
          for (final id in ids) _saved(id),
        ]),
      });

  List<String> shownIds() => [
    for (final channel in container.read(channelsProvider).value ?? const [])
      channel.id,
  ];

  test("the saved list shows before the relay answers, then the relay's "
      'list replaces it', () async {
    await saveList([_roomB]);
    session.memberships = {_roomA};
    final hold = Completer<void>();
    session.holdMemberships = hold;
    container.read(relaySessionProvider);
    container.listen(channelsProvider, (_, _) {});
    await _settle();

    final notifier = container.read(channelsProvider.notifier);
    expect(shownIds(), [_roomB]);
    expect(
      container.read(channelsProvider).value!.single.name,
      'saved-$_roomB',
    );
    expect(notifier.showsSavedList, isTrue);
    expect(notifier.hasLoaded, isFalse);
    expect(notifier.relayJoinedChannelIds, isEmpty);

    hold.complete();
    await _settle();
    expect(shownIds(), [_roomA]);
    expect(notifier.showsSavedList, isFalse);
    expect(notifier.hasLoaded, isTrue);
    expect(notifier.relayJoinedChannelIds, {_roomA});
  });

  test('the saved list never makes the purge forget a room', () async {
    // Room B is in the saved list only; no relay list ever had it.
    await saveList([_roomA, _roomB]);
    session.memberships = {_roomA};
    container.read(relaySessionProvider);
    store.submit(lease(), [
      _message('a1', _roomA, 100),
      _message('b1', _roomB, 100),
    ]);
    await container.read(channelsProvider.future);
    await _settle();
    for (var i = 0; i < 2; i++) {
      await container.read(channelsProvider.notifier).refresh();
      await _settle();
    }
    await store.flushed();
    expect(await _storedIds(store, lease()), ['a1', 'b1']);
  });

  test("the relay's list is saved for the next launch", () async {
    session.memberships = {_roomA};
    container.read(relaySessionProvider);
    container.listen(channelsProvider, (_, _) {});
    await container.read(channelsProvider.future);
    await _settle();
    await Future<void>.delayed(const Duration(milliseconds: 1200));
    await store.flushed();

    final saved = decodeChannelListSnapshot(
      await store.readSnapshot(lease(), channelListSnapshotKey),
    );
    expect([for (final channel in saved!) channel.id], [_roomA]);
  });

  test(
    "a failed relay list keeps the saved one, reports it and asks again",
    () async {
      await saveList([_roomB]);
      session.memberships = {_roomA};
      session.failMemberships = StateError('relay down');
      container.read(relaySessionProvider);
      container.listen(channelsProvider, (_, _) {});
      container.listen(channelListLoadErrorProvider, (_, _) {});
      await _settle();

      final notifier = container.read(channelsProvider.notifier);
      expect(shownIds(), [_roomB]);
      expect(notifier.showsSavedList, isTrue);
      expect(container.read(channelListLoadErrorProvider), isNotNull);

      session.failMemberships = null;
      await notifier.retryRelayList();
      await _settle();
      expect(shownIds(), [_roomA]);
      expect(notifier.showsSavedList, isFalse);
      expect(container.read(channelListLoadErrorProvider), isNull);
    },
  );

  test("an account switch never shows the previous account's rooms", () async {
    await saveList([_roomB], _other);
    session.memberships = {_roomA};
    container.read(relaySessionProvider);
    final seen = <List<String>>[];
    container.listen(channelsProvider, (_, next) {
      if (container.read(myPubkeyProvider) == _other) {
        seen.add([for (final c in next.value ?? const <Channel>[]) c.id]);
      }
    });
    await container.read(channelsProvider.future);
    await _settle();
    expect(shownIds(), [_roomA]);

    final hold = Completer<void>();
    session.holdMemberships = hold;
    session.memberships = {_roomB};
    container.read(_account.notifier).switchTo(_other);
    expect(shownIds(), isNot(contains(_roomA)));
    await _settle();
    // The other account's own saved list, then its relay list.
    expect(shownIds(), [_roomB]);
    expect(
      container.read(channelsProvider).value!.single.name,
      'saved-$_roomB',
    );
    hold.complete();
    await _settle();
    expect(shownIds(), [_roomB]);
    expect(container.read(channelsProvider).value!.single.name, _roomB);
    expect(seen.where((ids) => ids.contains(_roomA)), isEmpty);
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

  Object? failMemberships;

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
      if (failMemberships case final error?) throw error;
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
