import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/features/channels/channel_head_cache.dart';
import 'package:kuzz/features/channels/channel_room_store.dart';
import 'package:kuzz/shared/message_db/coverage.dart';
import 'package:kuzz/shared/message_db/message_db.dart';
import 'package:kuzz/shared/message_db/message_db_writer.dart';
import 'package:kuzz/shared/message_db/message_store.dart';
import 'package:kuzz/shared/relay/relay.dart';
import 'package:sqlite3/sqlite3.dart';

const _roomA = 'room-a';
const _roomB = 'room-b';
const _scope = ChannelHeadScope(relayUrl: 'https://relay.test', pubkey: 'me');
const _messageScope = MessageScope(
  relayUrl: 'https://relay.test',
  pubkey: 'me',
);

/// A message store whose writes can be made to fail.
class _FlakyMessageStore extends MessageStore {
  _FlakyMessageStore(super.root);

  bool failWrites = false;

  @override
  Future<void> write(
    MessageScope scope,
    MessageDbWrite write, {
    int? generation,
  }) {
    if (failWrites) {
      return Future.error(const MessageDbWriteException('disk full'));
    }
    return super.write(scope, write, generation: generation);
  }
}

/// Every stored event id, in order.
class _IdsRead extends MessageDbRead<List<String>> {
  const _IdsRead();

  @override
  List<String> run(Database db) => [
    for (final row in db.select('SELECT id FROM events ORDER BY id'))
      row['id'] as String,
  ];
}

NostrEvent _event(String id, String room, {int createdAt = 100}) => NostrEvent(
  id: id,
  pubkey: 'alice',
  createdAt: createdAt,
  kind: EventKind.streamMessageV2,
  tags: [
    ['h', room],
  ],
  content: id,
);

/// The reconnect catch-up's state in the message database: watermarks per
/// physical server and live ranges per fed room.
void main() {
  late Directory root;
  late _FlakyMessageStore messages;
  late ChannelRoomStore store;
  late int now;

  setUp(() {
    root = Directory.systemTemp.createTempSync('room_store_catch_up_test');
    messages = _FlakyMessageStore(() async => '${root.path}/messages');
    now = 1000;
    store = ChannelRoomStore(
      ChannelHeadCache(() async => Directory('${root.path}/heads')),
      messages: messages,
      clock: () => now,
    );
    store.setFedRooms(_scope, {_roomA, _roomB});
  });

  tearDown(() async {
    messages.failWrites = false;
    await store.flush();
    await messages.close();
    root.deleteSync(recursive: true);
  });

  Future<int?> liveFrom(String room) =>
      messages.read<int?>(_messageScope, LiveFromRead(room));

  Future<List<String>> ids() async =>
      await messages.read(_messageScope, const _IdsRead()) ?? const [];

  test('a fetched newest page starts a fed room\'s live range', () async {
    expect(await store.hasGap(_scope, _roomA), isTrue);

    store.clearGap(_scope, _roomA);
    expect(await store.hasGap(_scope, _roomA), isFalse);
    await store.flush();
    expect(await liveFrom(_roomA), 1000);

    // An open range is kept.
    now = 2000;
    store.clearGap(_scope, _roomA);
    await store.flush();
    expect(await liveFrom(_roomA), 1000);
  });

  test('a room the feed does not cover gets no live range', () async {
    store.clearGap(_scope, 'elsewhere');
    store.ingest(_scope, _event('x', 'elsewhere'));
    await store.flush();

    expect(await liveFrom('elsewhere'), isNull);
    expect(await store.hasGap(_scope, 'elsewhere'), isTrue);
  });

  test(
    'a marked gap ends the live range: a hole until the next page',
    () async {
      store.clearGap(_scope, _roomA);
      await store.flush();

      store.markGap(_scope, {_roomA});
      expect(await store.hasGap(_scope, _roomA), isTrue);
      await store.flush();
      expect(await liveFrom(_roomA), isNull);

      now = 3000;
      store.clearGap(_scope, _roomA);
      await store.flush();
      expect(await liveFrom(_roomA), 3000);
    },
  );

  test('a room dropped from the feed loses its live range', () async {
    store.clearGap(_scope, _roomA);
    store.clearGap(_scope, _roomB);
    await store.flush();

    store.setFedRooms(_scope, {_roomB});
    await store.flush();

    expect(await liveFrom(_roomA), isNull);
    expect(await liveFrom(_roomB), 1000);
  });

  test('a watermark is stored with the events queued before it', () async {
    expect(await store.loadWatermark(_scope), isNull);
    store.ingest(_scope, _event('a', _roomA, createdAt: 990));
    store.advanceWatermark(_scope, 990);
    // Never moves back.
    store.advanceWatermark(_scope, 900);
    await store.flush();

    expect(await ids(), ['a']);
    expect(await store.loadWatermark(_scope), 990);
    // Keyed by the physical server.
    const other = ChannelHeadScope(
      relayUrl: 'https://relay-2.test',
      pubkey: 'me',
    );
    expect(await store.loadWatermark(other), isNull);
  });

  test('a catch-up commits its events, holes and watermark together', () async {
    store.clearGap(_scope, _roomA);
    store.clearGap(_scope, _roomB);
    await store.flush();

    store.ingest(_scope, _event('missed-b', _roomB));
    final committed = await store.commitCatchUp(
      _scope,
      rooms: {_roomA, _roomB},
      gaps: {_roomA},
      watermark: 1500,
    );

    expect(committed, isTrue);
    expect(await ids(), ['missed-b']);
    expect(await liveFrom(_roomA), isNull);
    expect(await liveFrom(_roomB), 1000);
    expect(await store.loadWatermark(_scope), 1500);
  });

  test('a failed catch-up commit stores nothing and keeps the watermark; '
      'the next write ends the rooms\' live ranges first', () async {
    store.clearGap(_scope, _roomA);
    store.clearGap(_scope, _roomB);
    store.advanceWatermark(_scope, 1000);
    await store.flush();

    messages.failWrites = true;
    store.ingest(_scope, _event('missed-a', _roomA));
    final committed = await store.commitCatchUp(
      _scope,
      rooms: {_roomA, _roomB},
      gaps: const {},
      watermark: 1500,
    );
    messages.failWrites = false;

    expect(committed, isFalse);
    expect(await ids(), isEmpty);
    expect(await store.loadWatermark(_scope), 1000);
    // The lost events' room is a hole from now on, even before the write.
    expect(await store.hasGap(_scope, _roomA), isTrue);

    await store.flush();
    expect(await liveFrom(_roomA), isNull);
    expect(await liveFrom(_roomB), 1000);
    // The watermark rides along with the hole.
    expect(await store.loadWatermark(_scope), 1500);
  });

  test('a catch-up ends the ranges of rooms it was not for', () async {
    store.clearGap(_scope, _roomA);
    store.clearGap(_scope, _roomB);
    await store.flush();

    // Room B left the feed while the app was closed.
    final committed = await store.commitCatchUp(
      _scope,
      rooms: {_roomA},
      gaps: const {},
      watermark: 1500,
    );

    expect(committed, isTrue);
    expect(await liveFrom(_roomA), 1000);
    expect(await liveFrom(_roomB), isNull);
  });

  test(
    'ranges nothing stored vouches for end before a first watermark',
    () async {
      store.clearGap(_scope, _roomA);
      await store.flush();

      store.resetLiveRanges(_scope);
      expect(await store.hasGap(_scope, _roomA), isTrue);
      store.advanceWatermark(_scope, 2000);
      await store.flush();

      expect(await liveFrom(_roomA), isNull);
      expect(await store.loadWatermark(_scope), 2000);
    },
  );

  test('a restarted store reads the watermark back', () async {
    store.advanceWatermark(_scope, 1234);
    await store.flush();
    await messages.close();

    final reopened = MessageStore(() async => '${root.path}/messages');
    addTearDown(reopened.close);
    final restarted = ChannelRoomStore(
      ChannelHeadCache(() async => Directory('${root.path}/heads')),
      messages: reopened,
    );

    expect(await restarted.loadWatermark(_scope), 1234);
  });
}
