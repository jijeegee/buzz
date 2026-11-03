import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:sqflite_common_ffi/sqflite_ffi.dart';

import 'package:kuzz/shared/community/community.dart';
import 'package:kuzz/shared/message_store/message_store.dart';
import 'package:kuzz/shared/push/push_inbox.dart';
import 'package:kuzz/shared/push/push_inbox_drain.dart';
import 'package:kuzz/shared/relay/nostr_models.dart';

const _relay = 'wss://relay.example';
final _account = 'a' * 64;
final _token = 'ab' * 16;

void main() {
  sqfliteFfiInit();
  late Directory dir;
  late Directory root;
  late MessageStore store;
  late MessageStoreLease lease;

  setUp(() async {
    dir = await Directory.systemTemp.createTemp('push-inbox-drain');
    root = await Directory('${dir.path}/push-inbox').create();
    store = MessageStore(
      factory: () => databaseFactoryFfi,
      directory: () async => '${dir.path}/store',
    );
    lease = store.lease(_relay, _account)!;
  });

  tearDown(() async {
    await store.forget(_relay, _account);
    await dir.delete(recursive: true);
  });

  PushInboxDrainScope scope({String relayUrl = _relay}) =>
      (lease: lease, token: _token, relayUrl: relayUrl);

  Map<String, dynamic> event(int n, {String room = 'r', int kind = 9}) => {
    'id': n.toRadixString(16).padLeft(64, '0'),
    'pubkey': 'b' * 64,
    'created_at': 1000 + n,
    'kind': kind,
    'tags': [
      ['h', room],
    ],
    'content': 'm$n',
    'sig': '0' * 128,
  };

  File inboxFile(
    String name,
    List<Map<String, dynamic>> events, {
    String relayUrl = _relay,
    String? account,
  }) {
    final folder = Directory('${root.path}/$_token')
      ..createSync(recursive: true);
    return File('${folder.path}/$name')..writeAsStringSync(
      jsonEncode({
        'v': 1,
        'relayUrl': relayUrl,
        'account': account ?? _account,
        'fetchedAt': DateTime.now().millisecondsSinceEpoch ~/ 1000,
        'events': events,
      }),
    );
  }

  Future<void> keep(Set<String> rooms) async {
    store.forgetRoomsExcept(store.capture(lease), lease, rooms);
    await store.flushed();
  }

  Future<List<String>> storedIds() async {
    final db = await databaseFactoryFfi.openDatabase(
      await store.pathOf(lease),
      options: OpenDatabaseOptions(singleInstance: false),
    );
    try {
      return [
        for (final row in await db.rawQuery(
          'SELECT id FROM events ORDER BY id',
        ))
          row['id']! as String,
      ];
    } finally {
      await db.close();
    }
  }

  test('written files are folded, profiles cached, then deleted', () async {
    await keep({'r'});
    final file = inboxFile('1700000000000-a.json', [
      event(1),
      event(2, room: 'elsewhere'),
      event(3, kind: 0),
    ]);
    final folded = <List<String>>[];
    final profiles = <String>[];
    final deleted = await drainPushInbox(
      store: store,
      scope: scope(),
      root: root.path,
      current: () => true,
      folds: [
        (l, events) {
          expect(l, lease);
          folded.add([for (final e in events) e.id]);
        },
      ],
      cacheProfile: (NostrEvent p) => profiles.add(p.id),
    );
    expect(deleted, 1);
    expect(file.existsSync(), isFalse);
    expect(folded, [
      [event(1)['id']],
    ], reason: 'only what the store accepted');
    expect(profiles, [event(3)['id']]);
    expect(await storedIds(), [event(1)['id']]);
  });

  test('a dropped write keeps its file for the next drain', () async {
    await keep({'r'});
    final file = inboxFile('1700000000000-a.json', [event(1)]);
    final ended = lease;
    // The lease ends (sign-out): its writes are dropped.
    await store.forget(_relay, _account);
    final folded = <NostrEvent>[];
    expect(
      await drainPushInbox(
        store: store,
        scope: (lease: ended, token: _token, relayUrl: _relay),
        root: root.path,
        current: () => true,
        folds: [(_, events) => folded.addAll(events)],
        cacheProfile: (_) {},
      ),
      0,
    );
    expect(file.existsSync(), isTrue);
    expect(folded, isEmpty);

    // A drain under the next lease takes it.
    lease = store.lease(_relay, _account)!;
    await keep({'r'});
    expect(
      await drainPushInbox(
        store: store,
        scope: scope(),
        root: root.path,
        current: () => true,
        folds: const [],
        cacheProfile: (_) {},
      ),
      1,
    );
    expect(file.existsSync(), isFalse);
    expect(await storedIds(), [event(1)['id']]);
  });

  test('files for another server or account are deleted unread; uppercase '
      'host is the same server', () async {
    await keep({'r'});
    final other = inboxFile('1-a.json', [
      event(1),
    ], relayUrl: 'wss://other.example');
    final someone = inboxFile('2-a.json', [event(2)], account: 'c' * 64);
    final same = inboxFile('3-a.json', [
      event(3),
    ], relayUrl: 'WSS://RELAY.Example/');
    await drainPushInbox(
      store: store,
      scope: scope(),
      root: root.path,
      current: () => true,
      folds: const [],
      cacheProfile: (_) {},
    );
    expect([other, someone, same].map((f) => f.existsSync()), [
      false,
      false,
      false,
    ]);
    expect(await storedIds(), [event(3)['id']]);
  });

  test('a drain that outlives its lease folds nothing', () async {
    await keep({'r'});
    final file = inboxFile('1-a.json', [event(1), event(2, kind: 0)]);
    var folds = 0;
    var profiles = 0;
    await drainPushInbox(
      store: store,
      scope: scope(),
      root: root.path,
      current: () => false,
      folds: [(_, _) => folds++],
      cacheProfile: (_) => profiles++,
    );
    expect(folds, 0);
    expect(profiles, 0);
    expect(file.existsSync(), isFalse, reason: 'it is in the store');
  });

  test('a missing folder is no error', () async {
    expect(
      await drainPushInbox(
        store: store,
        scope: scope(),
        root: root.path,
        current: () => true,
        folds: const [],
        cacheProfile: (_) {},
      ),
      0,
    );
  });

  test('the sweep deletes token folders no community holds', () async {
    final live = Directory('${root.path}/$_token')..createSync();
    final orphan = Directory('${root.path}/${'cd' * 16}')..createSync();
    final other = Directory('${root.path}/not-a-token')..createSync();
    await sweepPushInbox(root.path, {_token});
    expect(live.existsSync(), isTrue);
    expect(orphan.existsSync(), isFalse);
    expect(other.existsSync(), isTrue);
  });

  test('sign-out and removal delete the folder', () async {
    final folder = Directory('${root.path}/$_token')..createSync();
    await deletePushInboxFolders(() async => root.path, [null, _token, '../x']);
    expect(folder.existsSync(), isFalse);
    expect(root.existsSync(), isTrue);
  });

  test('no drain while age-restricted, signed out or without a token', () {
    Community community({bool out = false, String? token}) => Community(
      id: 'c1',
      name: 'C',
      relayUrl: _relay,
      pubkey: _account,
      pushNotificationsEnabled: true,
      pushInboxToken: token,
      signedOut: out,
      addedAt: DateTime.fromMillisecondsSinceEpoch(0),
    );
    PushInboxDrainScope? scopeFor(Community? c, {bool restricted = false}) =>
        pushInboxDrainScope(
          ageRestricted: restricted,
          community: c,
          relayUrl: _relay,
          account: _account,
          store: store,
        );
    expect(scopeFor(community(token: _token))?.token, _token);
    expect(scopeFor(community(token: _token), restricted: true), isNull);
    expect(scopeFor(community(token: _token, out: true)), isNull);
    expect(scopeFor(community()), isNull);
    expect(scopeFor(null), isNull);
  });
}
