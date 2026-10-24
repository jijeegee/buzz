import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/features/channels/channel_head_cache.dart';
import 'package:kuzz/features/channels/channel_room_store.dart';
import 'package:kuzz/features/channels/timeline_message.dart';
import 'package:kuzz/shared/relay/relay.dart';

/// Live events for every joined room land in that room's stored tail, whether
/// or not the room is open.
void main() {
  group('applyLiveEvents', () {
    test(
      'a first event starts a tail complete from when the feed went live',
      () {
        final tail = applyLiveEvents(RoomTail.empty, [
          _live(_row('a', createdAt: 120), liveSince: 100, at: 121),
        ]);

        expect(_ids(tail.rows), ['a']);
        expect(tail.contiguousFrom, 100);
        expect(tail.syncedThrough, 121);
        expect(tail.gap, isFalse);
      },
    );

    test('keeps the newest rows only', () {
      final tail = applyLiveEvents(RoomTail.empty, [
        for (var i = 0; i < roomTailMaxRows + 5; i++)
          _live(_row('r$i', createdAt: 1000 + i)),
      ]);

      expect(tail.rows, hasLength(roomTailMaxRows));
      expect(tail.rows.first.id, 'r5');
      expect(tail.rows.last.id, 'r${roomTailMaxRows + 4}');
      expect(tail.contiguousFrom, 1005);
    });

    test('replies, reactions and huddle joins are not rows', () {
      final tail = applyLiveEvents(RoomTail.empty, [
        _live(_row('root', createdAt: 10)),
        _live(
          _row(
            'reply',
            createdAt: 11,
            tags: [
              ['e', 'root', '', 'reply'],
            ],
          ),
        ),
        _live(_aux('like', EventKind.reaction, target: 'root')),
        _live(_aux('join', EventKind.huddleParticipantJoined)),
      ]);

      expect(_ids(tail.rows), ['root']);
      expect(tail.aux, isEmpty);
    });

    test('edits overlay their row and the newest edit wins', () {
      final tail = applyLiveEvents(RoomTail.empty, [
        _live(_row('a', createdAt: 10, content: 'first')),
        _live(_edit('e2', target: 'a', createdAt: 30, content: 'third')),
        _live(_edit('e1', target: 'a', createdAt: 20, content: 'second')),
        _live(_edit('e3', target: 'missing', createdAt: 40)),
      ]);

      expect(_ids(tail.aux), ['e2']);
      expect(formatTimeline(tail.events).single.content, 'third');
    });

    test('a deletion removes the row and its overlays', () {
      final tail = applyLiveEvents(RoomTail.empty, [
        _live(_row('a', createdAt: 10)),
        _live(_row('b', createdAt: 11)),
        _live(_edit('edit', target: 'a', createdAt: 12)),
        _live(_summary('a', replies: 2)),
        _live(_aux('del', EventKind.nip29DeleteEvent, target: 'a')),
      ]);

      expect(_ids(tail.rows), ['b']);
      expect(tail.aux, isEmpty);
    });

    test('the newest thread summary of a row is kept', () {
      final tail = applyLiveEvents(RoomTail.empty, [
        _live(_row('a', createdAt: 10)),
        _live(_summary('a', replies: 1, createdAt: 20)),
        _live(_summary('a', replies: 3, createdAt: 30)),
      ]);

      expect(tail.aux.single.content, contains('"reply_count":3'));
    });

    test('a feed that reconnected after the tail was synced marks a gap', () {
      final synced = applyLiveEvents(RoomTail.empty, [
        _live(_row('a', createdAt: 10), liveSince: 5, at: 10),
      ]);
      final continuous = applyLiveEvents(synced, [
        _live(_row('b', createdAt: 20), liveSince: 5, at: 20),
      ]);
      expect(continuous.gap, isFalse);

      final reconnected = applyLiveEvents(continuous, [
        _live(_row('c', createdAt: 40), liveSince: 30, at: 40),
      ]);
      expect(reconnected.gap, isTrue);
      expect(reconnected.syncedThrough, 40);
      expect(_ids(reconnected.rows), ['a', 'b', 'c']);
    });

    test('a newer relay page closes the gap and keeps later arrivals', () {
      final tail = applyLiveEvents(RoomTail.empty, [
        _live(_row('old', createdAt: 10)),
        _live(_row('late', createdAt: 50)),
        _live(_edit('edit', target: 'late', createdAt: 51)),
      ], markGap: true);

      final fromHead = roomTailFromHead(
        [_row('page', createdAt: 40), _bounds(hasMore: false)],
        tail,
        now: 60,
      );

      expect(_ids(fromHead.rows), ['page', 'late']);
      expect(_ids(fromHead.aux), ['edit']);
      expect(fromHead.contiguousFrom, 0);
      expect(fromHead.syncedThrough, 60);
      expect(fromHead.gap, isFalse);
    });
  });

  group('ChannelRoomStore', () {
    late Directory root;
    late ChannelHeadCache cache;
    late ChannelRoomStore store;
    var now = 1000;

    setUp(() async {
      root = await Directory.systemTemp.createTemp('channel_room_store_test');
      cache = ChannelHeadCache(() async => root);
      now = 1000;
      store = ChannelRoomStore(cache, clock: () => now)..markLive();
    });

    tearDown(() async {
      await store.flush();
      if (await root.exists()) await root.delete(recursive: true);
    });

    test('events for a room with no head persist as its tail', () async {
      store.ingest(_scope, _row('a', createdAt: 1001));
      store.ingest(_scope, _row('b', createdAt: 1002));
      // Writes wait for the batch.
      expect(await cache.loadTail(_scope, _channelId), isNull);

      await store.flush();

      final reopened = ChannelHeadCache(() async => root);
      final tail = await reopened.loadTail(_scope, _channelId);
      expect(_ids(tail!.rows), ['a', 'b']);
      expect(tail.contiguousFrom, 1000);
      // A room known only from live events has nothing to paint as a head.
      expect(await reopened.load(_scope, _channelId), isNull);
      expect(await reopened.storedAt(_scope, _channelId), isNull);
      expect(await reopened.loadTail(_otherAccount, _channelId), isNull);
    });

    test('live writes keep the head and its store time', () async {
      await cache.store(_scope, _channelId, [
        _row('head', createdAt: 900),
        _bounds(hasMore: true),
      ]);
      final storedAt = await cache.storedAt(_scope, _channelId);

      store.ingest(_scope, _row('live', createdAt: 1001));
      store.ingest(_scope, _edit('edit', target: 'head', createdAt: 1002));
      await store.flush();

      expect(_ids((await cache.load(_scope, _channelId))!), ['head', 'bounds']);
      expect(await cache.storedAt(_scope, _channelId), storedAt);
      final tail = await cache.loadTail(_scope, _channelId);
      expect(_ids(tail!.rows), ['head', 'live']);
      expect(_ids(tail.aux), ['edit']);
      expect(tail.contiguousFrom, 900);
      // Rows the head already holds are stored once.
      final file = root
          .listSync(recursive: true)
          .whereType<File>()
          .single
          .readAsStringSync();
      expect(jsonDecode(file)['tail'].first, {'ref': 'head'});
    });

    test('deletions and edits reach stored rows', () async {
      store.ingest(_scope, _row('a', createdAt: 1001, content: 'hi'));
      store.ingest(_scope, _row('b', createdAt: 1002));
      await store.flush();

      store.ingest(_scope, _edit('edit', target: 'a', content: 'hello'));
      store.ingest(_scope, _aux('del', EventKind.deletion, target: 'b'));
      await store.flush();

      final tail = await cache.loadTail(_scope, _channelId);
      expect(_ids(tail!.rows), ['a']);
      expect(formatTimeline(tail.events).single.content, 'hello');
    });

    test('a tail stored before a reconnect has a gap', () async {
      store.ingest(_scope, _row('a', createdAt: 1001));
      await store.flush();

      now = 2000;
      store.markLive();
      store.ingest(_scope, _row('b', createdAt: 2001));
      await store.flush();

      expect((await cache.loadTail(_scope, _channelId))!.gap, isTrue);
    });

    test('a subscription requested after the last write marks a gap', () async {
      store.ingest(_scope, _row('a', createdAt: 1001));
      await store.flush();

      now = 1500;
      store.ingest(_scope, _row('b', createdAt: 1501), liveSince: 1400);
      await store.flush();

      expect((await cache.loadTail(_scope, _channelId))!.gap, isTrue);
    });

    test('events queued before a clear are dropped', () async {
      store.ingest(_scope, _row('a', createdAt: 1001));
      await cache.clearAll();
      await store.flush();

      expect(await cache.loadTail(_scope, _channelId), isNull);
    });

    test('a schema 1 entry reads as empty and is replaced', () async {
      await cache.store(_scope, _channelId, [_row('head', createdAt: 900)]);
      final file = root.listSync(recursive: true).whereType<File>().single;
      file.writeAsStringSync(
        jsonEncode({
          'v': 1,
          'stored_at': 900,
          'channel': _channelId,
          'events': [_row('head', createdAt: 900).toJson()],
        }),
      );

      expect(await cache.loadTail(_scope, _channelId), isNull);
      store.ingest(_scope, _row('live', createdAt: 1001));
      await store.flush();
      expect(_ids((await cache.loadTail(_scope, _channelId))!.rows), ['live']);
      expect(await cache.load(_scope, _channelId), isNull);
    });
  });
}

const _channelId = '11111111-1111-4111-8111-111111111111';
const _scope = ChannelHeadScope(
  relayUrl: 'https://relay.example',
  pubkey: 'alice',
);
const _otherAccount = ChannelHeadScope(
  relayUrl: 'https://relay.example',
  pubkey: 'bob',
);

List<String> _ids(List<NostrEvent> events) => [
  for (final event in events) event.id,
];

RoomLiveEvent _live(NostrEvent event, {int liveSince = 0, int? at}) =>
    RoomLiveEvent(
      event,
      liveSince: liveSince,
      receivedAt: at ?? event.createdAt,
    );

NostrEvent _row(
  String id, {
  int createdAt = 1,
  String? content,
  List<List<String>> tags = const [],
}) => NostrEvent(
  id: id,
  pubkey: 'alice',
  createdAt: createdAt,
  kind: EventKind.streamMessageV2,
  tags: [
    ['h', _channelId],
    ...tags,
  ],
  content: content ?? id,
  sig: 'sig',
);

NostrEvent _aux(String id, int kind, {String? target, int createdAt = 100}) =>
    NostrEvent(
      id: id,
      pubkey: 'alice',
      createdAt: createdAt,
      kind: kind,
      tags: [
        ['h', _channelId],
        if (target != null) ['e', target],
      ],
      content: kind == EventKind.reaction ? '+' : '',
      sig: 'sig',
    );

NostrEvent _edit(
  String id, {
  required String target,
  int createdAt = 100,
  String content = 'edited',
}) => NostrEvent(
  id: id,
  pubkey: 'alice',
  createdAt: createdAt,
  kind: EventKind.streamMessageEdit,
  tags: [
    ['h', _channelId],
    ['e', target],
  ],
  content: content,
  sig: 'sig',
);

NostrEvent _summary(
  String rootId, {
  required int replies,
  int createdAt = 50,
}) => NostrEvent(
  id: 'summary-$rootId-$createdAt',
  pubkey: 'relay',
  createdAt: createdAt,
  kind: EventKind.channelThreadSummary,
  tags: [
    ['h', _channelId],
    ['d', rootId],
    ['e', rootId],
  ],
  content: jsonEncode({'reply_count': replies, 'descendant_count': replies}),
  sig: 'sig',
);

NostrEvent _bounds({required bool hasMore}) => NostrEvent(
  id: 'bounds',
  pubkey: 'relay',
  createdAt: 0,
  kind: EventKind.channelWindowBounds,
  tags: [
    ['d', '$_channelId:head'],
  ],
  content: jsonEncode({'has_more': hasMore, 'next_cursor': null}),
  sig: 'sig',
);
