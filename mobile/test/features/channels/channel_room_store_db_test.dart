import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/features/channels/channel_head_cache.dart';
import 'package:kuzz/features/channels/channel_page_records.dart';
import 'package:kuzz/features/channels/channel_room_store.dart';
import 'package:kuzz/features/channels/channel_window.dart';
import 'package:kuzz/features/channels/thread_replies_provider.dart';
import 'package:kuzz/shared/message_db/coverage.dart';
import 'package:kuzz/shared/message_db/message_db.dart';
import 'package:kuzz/shared/message_db/message_store.dart';
import 'package:kuzz/shared/relay/relay.dart';
import 'package:sqlite3/sqlite3.dart';

const _room = 'room-a';
const _scope = ChannelHeadScope(relayUrl: 'https://relay.test', pubkey: 'me');
const _messageScope = MessageScope(
  relayUrl: 'https://relay.test',
  pubkey: 'me',
);

/// ChannelRoomStore writes every received event to the message database,
/// next to (not instead of) the #288 stored tail; reads are unchanged.
void main() {
  late Directory root;
  late MessageStore messages;
  late ChannelRoomStore store;
  late int now;

  setUp(() {
    root = Directory.systemTemp.createTempSync('room_store_db_test');
    messages = MessageStore(() async => '${root.path}/messages');
    now = 1000;
    store = ChannelRoomStore(
      ChannelHeadCache(() async => Directory('${root.path}/heads')),
      messages: messages,
      clock: () => now,
    );
  });

  tearDown(() async {
    await store.flush();
    await messages.close();
    root.deleteSync(recursive: true);
  });

  T read<T>(T Function(Database db) query) {
    final db = openMessageDbReader(
      '${root.path}/messages/${_messageScope.relativePath}',
    );
    try {
      return query(db);
    } finally {
      db.close();
    }
  }

  List<String> ids() => read(
    (db) => [
      for (final row in db.select('SELECT id FROM events ORDER BY id'))
        row['id'] as String,
    ],
  );

  List<(String, int, int)> coverage() => read(
    (db) => [
      for (final row in db.select(
        'SELECT stream, lo_at, hi_at FROM coverage ORDER BY stream, lo_at',
      ))
        (row['stream'] as String, row['lo_at'] as int, row['hi_at'] as int),
    ],
  );

  test('live events of every kind are stored, without coverage', () async {
    store.markLive();
    store.ingest(_scope, _event('msg', createdAt: 10));
    store.ingest(
      _scope,
      _event(
        'like',
        kind: EventKind.reaction,
        tags: [
          ['e', 'msg'],
        ],
      ),
    );
    store.ingest(
      _scope,
      _event('join', kind: EventKind.huddleParticipantJoined),
    );
    await store.flush();

    expect(ids(), ['join', 'like', 'msg']);
    expect(coverage(), isEmpty);
    // The reconnect time is recorded for this physical server.
    expect(
      read(
        (db) => db.select('SELECT origin, live_since FROM sync_state'),
      ).map((row) => (row['origin'], row['live_since'])),
      [('https://relay.test', 1000)],
    );
    // The #288 tail is unchanged: it keeps rows only.
    final tail = await ChannelHeadCache(
      () async => Directory('${root.path}/heads'),
    ).loadTail(_scope, _room);
    expect(tail!.rows.map((event) => event.id), ['msg']);
  });

  test('events loaded without a page are stored without coverage', () async {
    store.record(_scope, [_event('deep-link', createdAt: 5)]);
    await store.flush();

    expect(ids(), ['deep-link']);
    expect(coverage(), isEmpty);
  });

  test('a clear while live events wait drops them', () async {
    store.ingest(_scope, _event('msg'));

    await messages.clear(_messageScope);
    await store.flush();

    expect(
      File('${root.path}/messages/${_messageScope.relativePath}').existsSync(),
      isFalse,
    );
  });

  test('a newest page stores its rows, overlays and coverage', () async {
    final response = [
      _event('new', createdAt: 30),
      _event('old', createdAt: 10),
      _event(
        'edit',
        kind: EventKind.streamMessageEdit,
        tags: [
          ['e', 'old'],
        ],
      ),
      _bounds(hasMore: true, next: (10, 'old')),
    ];
    final page = parseChannelWindowResponse(response, _room, null);

    await store.recordPage(
      _scope,
      response,
      channelId: _room,
      claims: [?channelWindowPageClaim(_room, page)],
    );

    expect(ids(), ['edit', 'new', 'old']);
    expect(coverage(), [(CoverageStream.main, 10, 30)]);
  });

  test('an older page chained from the newest one extends its range', () async {
    final head = [
      _event('b', createdAt: 20),
      _bounds(hasMore: true, next: (20, 'b')),
    ];
    final older = [
      _event('a', createdAt: 10),
      _bounds(hasMore: false, cursor: (20, 'b')),
    ];
    for (final (response, cursor) in [
      (head, null),
      (older, const ChannelPageCursor(createdAt: 20, eventId: 'b')),
    ]) {
      final page = parseChannelWindowResponse(response, _room, cursor);
      await store.recordPage(
        _scope,
        response,
        channelId: _room,
        claims: [?channelWindowPageClaim(_room, page)],
      );
    }

    // Reaches the room's first message.
    expect(coverage(), [(CoverageStream.main, -1, 20)]);
  });

  group('thread claims', () {
    const thread = ThreadRepliesArgs(channelId: _room, rootId: 'root');

    test('a complete scan covers both thread streams to its newest '
        'second', () {
      final claims = completeThreadClaims(thread, [
        _event('r1', createdAt: 10),
        _event('r2', createdAt: 40),
      ]);

      expect(claims.map((claim) => claim.stream), [
        CoverageStream.thread('root'),
        CoverageStream.threadWindow('root'),
      ]);
      expect(
        claims.first.range,
        const CoverageRange(
          CoverageCursor.start,
          CoverageCursor.endOfSecond(40),
        ),
      );
      expect(completeThreadClaims(thread, const []), isEmpty);
    });
  });
}

NostrEvent _event(
  String id, {
  int kind = EventKind.streamMessage,
  int createdAt = 1,
  List<List<String>> tags = const [],
}) => NostrEvent(
  id: id,
  pubkey: 'author',
  createdAt: createdAt,
  kind: kind,
  tags: [
    ['h', _room],
    ...tags,
  ],
  content: 'text',
);

NostrEvent _bounds({
  required bool hasMore,
  (int, String)? next,
  (int, String)? cursor,
}) => NostrEvent(
  id: 'bounds-${cursor?.$2 ?? 'head'}',
  pubkey: 'relay',
  createdAt: 99,
  kind: EventKind.channelWindowBounds,
  tags: [
    ['d', cursor == null ? '$_room:head' : '$_room:${cursor.$1}:${cursor.$2}'],
    ['h', _room],
  ],
  content: jsonEncode({
    'has_more': hasMore,
    'next_cursor': next == null ? null : {'created_at': next.$1, 'id': next.$2},
  }),
);
