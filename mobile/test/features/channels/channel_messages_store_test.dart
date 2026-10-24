import 'dart:async';
import 'dart:collection';
import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:kuzz/features/channels/channel_head_cache.dart';
import 'package:kuzz/features/channels/channel_messages_provider.dart';
import 'package:kuzz/features/channels/channel_room_store.dart';
import 'package:kuzz/shared/message_db/coverage.dart';
import 'package:kuzz/shared/message_db/event_rows.dart';
import 'package:kuzz/shared/message_db/message_db.dart';
import 'package:kuzz/shared/message_db/message_store.dart';
import 'package:kuzz/shared/relay/relay.dart';

const _channelId = '11111111-1111-4111-8111-111111111111';
const _scope = ChannelHeadScope(relayUrl: 'https://relay.test', pubkey: 'me');
const _messageScope = MessageScope(
  relayUrl: 'https://relay.test',
  pubkey: 'me',
);

/// A room reads its timeline from the message database: the first build
/// paints stored rows, and the relay is asked only for what is not stored.
void main() {
  late Directory root;
  late MessageStore messages;
  late _Session session;
  late ProviderContainer container;

  setUp(() {
    root = Directory.systemTemp.createTempSync('channel_messages_store_test');
    messages = MessageStore(
      () async => '${root.path}/messages',
      rootPath: '${root.path}/messages',
    );
    session = _Session();
    container = ProviderContainer(
      overrides: [
        relaySessionProvider.overrideWith(() => session),
        channelHeadScopeProvider.overrideWithValue(_scope),
        channelHeadCacheProvider.overrideWithValue(
          ChannelHeadCache(() async => Directory('${root.path}/heads')),
        ),
        messageStoreProvider.overrideWithValue(messages),
      ],
    );
  });

  tearDown(() async {
    container.dispose();
    await messages.close();
    root.deleteSync(recursive: true);
  });

  Future<void> seed(
    List<NostrEvent> events, {
    List<(CoverageCursor, CoverageCursor)> ranges = const [],
    int? liveFrom,
  }) async {
    await messages.write(
      _messageScope,
      MessageDbWrite(
        rows: messageRowsFor(events, receivedAt: 1),
        claims: [
          for (final (lo, hi) in ranges)
            CoverageClaim(
              channelId: _channelId,
              stream: CoverageStream.main,
              range: CoverageRange(lo, hi),
            ),
        ],
        liveFrom: {_channelId: ?liveFrom},
      ),
    );
    await messages.settled();
  }

  void feedRoom() {
    container.read<ChannelRoomStore>(channelRoomStoreProvider)
      ..markLive()
      ..setFedRooms(_scope, {_channelId});
  }

  List<String> ids() => [
    for (final event
        in container.read(channelMessagesProvider(_channelId)).value!)
      event.id,
  ];

  CoverageCursor at(int createdAt, [String id = '']) =>
      CoverageCursor(createdAt, id);

  test('first build paints stored rows synchronously', () async {
    await seed(
      [_event('a', 10), _event('b', 20)],
      ranges: [(CoverageCursor.start, at(20, 'b'))],
    );
    final value = container.read(channelMessagesProvider(_channelId));
    expect(value, isA<AsyncData<List<NostrEvent>>>());
    expect([for (final e in value.value!) e.id], ['a', 'b']);
  });

  test('an empty database shows the loading skeleton', () {
    final value = container.read(channelMessagesProvider(_channelId));
    expect(value, isA<AsyncLoading<List<NostrEvent>>>());
  });

  test('stored rows outside a complete range are not painted', () async {
    await seed([_event('a', 10)]);
    final value = container.read(channelMessagesProvider(_channelId));
    expect(value, isA<AsyncLoading<List<NostrEvent>>>());
  });

  test('reopening a covered room sends no request', () async {
    await seed(
      [_event('a', 10), _event('b', 20)],
      ranges: [(CoverageCursor.start, at(20, 'b'))],
      liveFrom: 20,
    );
    feedRoom();
    container.read(channelMessagesProvider(_channelId));
    await session.subscribed;
    await _pump();

    expect(ids(), ['a', 'b']);
    expect(session.queryFilters, isEmpty);
    expect(session.historyFilters, isEmpty);
    expect(
      container
          .read(channelMessagesProvider(_channelId).notifier)
          .isShowingCachedHead,
      isFalse,
    );
  });

  test(
    'an unfed room asks only for rows since its newest stored row',
    () async {
      await seed(
        [_event('a', 10), _event('b', 20)],
        ranges: [(CoverageCursor.start, at(20, 'b'))],
      );
      session.queryResults.add([_event('c', 30)]);
      container.read(channelMessagesProvider(_channelId));
      await session.subscribed;
      await _pump();

      expect(session.queryFilters, hasLength(1));
      final filter = session.queryFilters.single;
      expect(filter.since, 20);
      expect(filter.until, isNull);
      // The scanned row lands below the stored ones; nothing moves.
      expect(ids(), ['a', 'b', 'c']);
      expect(
        container
            .read(channelMessagesProvider(_channelId).notifier)
            .isShowingCachedHead,
        isFalse,
      );
    },
  );

  test('scrolling up across a hole asks the relay only at the hole', () async {
    await seed(
      [_event('a', 10), _event('b', 20), _event('c', 30), _event('d', 40)],
      ranges: [(CoverageCursor.start, at(20, 'b')), (at(30, 'c'), at(40, 'd'))],
      liveFrom: 40,
    );
    feedRoom();
    final notifier = container.read(
      channelMessagesProvider(_channelId).notifier,
    );
    await session.subscribed;
    await _pump();
    expect(ids(), ['c', 'd']);
    expect(session.queryFilters, isEmpty);

    // The hole below c: one relay page, which reaches the stored range.
    session.queryResults.add([
      _event('b', 20),
      _bounds('$_channelId:30:c', hasMore: true, cursor: (20, 'b')),
    ]);
    expect(await notifier.fetchOlder(), isTrue);
    expect(session.queryFilters, hasLength(1));
    expect(session.queryFilters.single.until, 30);
    expect(session.queryFilters.single.extensions['before_id'], 'c');
    expect(ids(), ['b', 'c', 'd']);
    await messages.settled();

    // Below b the stored range continues: no request.
    expect(await notifier.fetchOlder(), isTrue);
    expect(session.queryFilters, hasLength(1));
    expect(ids(), ['a', 'b', 'c', 'd']);
    expect(notifier.reachedOldest, isTrue);
  });

  test('deleted rows do not come back', () async {
    await seed(
      [_event('a', 10), _event('b', 20), _deletion('del', 21, target: 'b')],
      ranges: [(CoverageCursor.start, at(21))],
      liveFrom: 21,
    );
    feedRoom();
    container.read(channelMessagesProvider(_channelId));
    expect(ids(), ['a']);
    await session.subscribed;
    await _pump();
    expect(ids(), ['a']);
    expect(session.queryFilters, isEmpty);
  });
}

NostrEvent _event(String id, int createdAt) => NostrEvent(
  id: id,
  pubkey: 'alice',
  createdAt: createdAt,
  kind: EventKind.streamMessageV2,
  tags: const [
    ['h', _channelId],
  ],
  content: id,
  sig: 'sig',
);

NostrEvent _deletion(String id, int createdAt, {required String target}) =>
    NostrEvent(
      id: id,
      pubkey: 'alice',
      createdAt: createdAt,
      kind: EventKind.deletion,
      tags: [
        const ['h', _channelId],
        ['e', target],
      ],
      content: '',
      sig: 'sig',
    );

NostrEvent _bounds(
  String dTag, {
  bool hasMore = false,
  (int, String)? cursor,
}) => NostrEvent(
  id: 'bounds-$dTag',
  pubkey: 'relay',
  createdAt: 0,
  kind: EventKind.channelWindowBounds,
  tags: [
    ['d', dTag],
  ],
  content: jsonEncode({
    'has_more': hasMore,
    'next_cursor': cursor == null
        ? null
        : {'created_at': cursor.$1, 'id': cursor.$2},
  }),
  sig: 'sig',
);

Future<void> _pump() async {
  for (var i = 0; i < 5; i++) {
    await Future<void>.delayed(Duration.zero);
  }
}

class _Session extends RelaySessionNotifier {
  final Queue<List<NostrEvent>> queryResults = Queue();
  final List<NostrFilter> queryFilters = [];
  final List<NostrFilter> historyFilters = [];
  final Completer<void> _subscribed = Completer();

  Future<void> get subscribed => _subscribed.future;

  @override
  SessionState build() => const SessionState(status: SessionStatus.connected);

  @override
  Future<List<NostrEvent>> queryRelay(
    List<NostrFilter> filters, {
    Duration timeout = const Duration(seconds: 8),
  }) async {
    queryFilters.addAll(filters);
    if (queryResults.isEmpty) return const [];
    return queryResults.removeFirst();
  }

  @override
  Future<List<NostrEvent>> fetchHistory(
    NostrFilter filter, {
    Duration timeout = const Duration(seconds: 8),
  }) async {
    historyFilters.add(filter);
    return const [];
  }

  @override
  Future<void Function()> subscribe(
    NostrFilter filter,
    void Function(NostrEvent) onEvent, {
    void Function(String message)? onClosed,
  }) async {
    if (!_subscribed.isCompleted) _subscribed.complete();
    return () {};
  }
}
