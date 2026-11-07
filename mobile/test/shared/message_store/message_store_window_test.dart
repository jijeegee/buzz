import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:sqflite_common_ffi/sqflite_ffi.dart';

import 'package:kuzz/shared/message_store/message_store.dart';
import 'package:kuzz/shared/relay/nostr_models.dart';
import 'package:kuzz/shared/relay/store_heads.dart';

const _relay = 'wss://relay.example';
const _account =
    'aa00000000000000000000000000000000000000000000000000000000000001';
const _room = 'room-1';
const _roomKinds = [9, 40002];
const _threadKinds = [9, 40002];

Map<String, dynamic> _row(String id, int at, {String? root}) => {
  'id': id,
  'kind': 9,
  'pubkey': 'p1',
  'created_at': at,
  'tags': [
    ['h', _room],
    if (root != null) ...[
      ['e', root, '', 'root'],
      ['e', root, '', 'reply'],
    ],
  ],
  'content': '',
};

/// The store side of checked windows (lib/shared/relay/window_check.dart):
/// the stored window a check compares, and the replacement of a fetched
/// page's key range.
void main() {
  sqfliteFfiInit();

  late Directory dir;
  late MessageStore store;
  late MessageStoreLease lease;

  setUp(() async {
    dir = await Directory.systemTemp.createTemp('message_store_window');
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

  StoreWindowKey room({(int, String)? before}) =>
      (room: _room, root: null, kinds: _roomKinds, before: before);

  Future<StoredCheckWindow> read(StoreWindowKey window, int limit) async =>
      (await store.checkWindow(lease, window, limit: limit))!;

  List<String> ids(StoredCheckWindow window) => [
    for (final row in window.rows) row['id'] as String,
  ];

  bool replace(
    StoreWindowKey window,
    List<String> pageIds, {
    (int, String)? oldest,
    required bool hasMore,
    StoreCapture? capture,
  }) => store.replaceWindow(capture ?? store.headSent(lease, [_room]), lease, (
    window: window,
    ids: pageIds.toSet(),
    oldest: oldest,
    hasMore: hasMore,
  ));

  test('a window holding as many rows as asked is fully stored', () async {
    store.submit(lease, [_row('a1', 1000), _row('a2', 2000), _row('a3', 3000)]);
    final full = await read(room(), 3);
    expect(ids(full), ['a3', 'a2', 'a1']);
    expect(full.oldest, (1000, 'a1'));
    expect(full.fullyStored, isTrue);
    // A short window is fully stored only once its start is.
    expect((await read(room(), 5)).fullyStored, isFalse);
  });

  test('a row deleted on the relay goes with its page', () async {
    store.submit(lease, [
      _row('a1', 1000),
      _row('a2', 2000),
      _row('a3', 3000),
      _row('a4', 4000),
    ]);
    expect(replace(room(), ['a4', 'a2', 'a1'], hasMore: false), isTrue);
    final window = await read(room(), 3);
    expect(ids(window), ['a4', 'a2', 'a1']);
    // The page had no more: the start is stored, a short window is whole.
    expect((await read(room(), 50)).fullyStored, isTrue);
  });

  test('an open lower bound leaves rows below the page alone', () async {
    store.submit(lease, [
      _row('a1', 1000),
      _row('a2', 2000),
      _row('a3', 3000),
      _row('a4', 4000),
    ]);
    replace(room(), ['a4', 'a2'], oldest: (2000, 'a2'), hasMore: true);
    expect(ids(await read(room(), 10)), ['a4', 'a2', 'a1']);
    // No start was stored by a page with more below it.
    expect((await read(room(), 10)).fullyStored, isFalse);
  });

  test('an older page replaces only its own range', () async {
    store.submit(lease, [
      _row('a1', 1000),
      _row('a2', 2000),
      _row('a3', 3000),
      _row('a4', 4000),
      _row('a5', 5000),
    ]);
    final older = room(before: (4000, 'a4'));
    replace(older, ['a3', 'a1'], hasMore: false);
    expect(ids(await read(room(), 10)), ['a5', 'a4', 'a3', 'a1']);
    expect(ids(await read(older, 2)), ['a3', 'a1']);
  });

  test('rows written after the page went out stay', () async {
    store.submit(lease, [_row('a1', 1000), _row('a2', 2000)]);
    final capture = store.headSent(lease, [_room]);
    store.submit(lease, [_row('a3', 3000)]);
    replace(room(), ['a2', 'a1'], hasMore: false, capture: capture);
    expect(ids(await read(room(), 10)), ['a3', 'a2', 'a1']);
  });

  test(
    'thread replies are no room rows; a thread window has its own',
    () async {
      store.submit(lease, [
        _row('a1', 1000),
        _row('t1', 1100, root: 'a1'),
        _row('t2', 1200, root: 'a1'),
      ]);
      expect(ids(await read(room(), 10)), ['a1']);
      final thread = (
        room: _room,
        root: 'a1',
        kinds: _threadKinds,
        before: null,
      );
      replace(thread, ['t2'], hasMore: false);
      final window = await read(thread, 10);
      expect(ids(window), ['t2']);
      expect(window.fullyStored, isTrue);
      expect(ids(await read(room(), 10)), ['a1']);
    },
  );

  test('a skipped catch-up leaves the room to its next newest page', () async {
    store.submit(lease, [_row('a1', 1000), _row('a2', 2000), _row('a3', 3000)]);
    store.markWindowsRefetch(lease, [_room]);
    expect((await read(room(), 3)).fullyStored, isFalse);
    // An older page does not clear it; the newest does.
    replace(room(before: (3000, 'a3')), ['a2', 'a1'], hasMore: false);
    expect((await read(room(), 3)).fullyStored, isFalse);
    replace(room(), ['a3', 'a2', 'a1'], hasMore: false);
    expect((await read(room(), 3)).fullyStored, isTrue);
  });

  test('the stored window answers in the shape of a relay page', () async {
    store.submit(lease, [_row('a1', 1000), _row('a2', 2000)]);
    final response = (await read(room(), 2)).toResponse(hasMore: true);
    expect(response.map((event) => event['id']).take(2), ['a2', 'a1']);
    final bounds = response.last;
    expect(bounds['kind'], 39006);
    expect((bounds['tags'] as List).first, ['d', '$_room:head']);
    expect(bounds['content'], contains('"has_more":true'));
  });

  test('an equal stored window records the head span a page would', () async {
    store.submit(lease, [_row('a1', 1000), _row('a2', 2000), _row('a3', 3000)]);
    StoreHeadAnswer answerOf(List<Map<String, dynamic>> response) =>
        storeHeadAnswers([
          (roomId: _room, rootId: null, kinds: _roomKinds),
        ], response.map(NostrEvent.fromJson).toList()).single;
    final stored = await read(room(), 3);
    // The stored window read the way queryRelayWithStoreHeads reads a page.
    final whole = answerOf(stored.toResponse(hasMore: false));
    expect(whole.hasMore, isFalse);
    expect(whole.ids, {'a1', 'a2', 'a3'});
    final more = answerOf(stored.toResponse(hasMore: true));
    expect(more.nextCursor, (1000, 'a1'));
    // The room's live REQ has been live since 10000.
    store.extendCoverage(lease, [_room], 10000, 20000);
    Future<Object?> certain() => store.certainRoomHead(
      lease,
      room: _room,
      kinds: _roomKinds,
      limit: 3,
      liveAt: 20000,
    );
    expect(await certain(), isNull);
    final mark = store.headSent(lease, [_room]);
    expect(
      store.recordHeads(mark, lease, [
        StoreHeadSpan(
          room: _room,
          root: null,
          kinds: _roomKinds,
          liveAt: 10000,
          oldest: whole.nextCursor,
          ids: whole.ids,
        ),
      ]),
      isTrue,
    );
    // The room is now certain, as after a fetched page.
    expect(await certain(), isNotNull);
  });

  test('a stored thread window is read as its page too', () async {
    store.submit(lease, [_row('a1', 1000), _row('t1', 1100, root: 'a1')]);
    final thread = (room: _room, root: 'a1', kinds: _threadKinds, before: null);
    final response = (await read(thread, 50)).toResponse(hasMore: false);
    final answer = storeHeadAnswers([
      (roomId: _room, rootId: 'a1', kinds: _threadKinds),
    ], response.map(NostrEvent.fromJson).toList()).single;
    expect(answer.ids, contains('t1'));
    expect(answer.hasMore, isFalse);
  });
}
