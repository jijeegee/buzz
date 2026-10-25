import 'dart:collection';
import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:kuzz/features/channels/channel_room_store.dart';
import 'package:kuzz/features/channels/thread_replies_provider.dart';
import 'package:kuzz/features/channels/thread_window.dart';
import 'package:kuzz/shared/message_db/coverage.dart';
import 'package:kuzz/shared/message_db/event_rows.dart';
import 'package:kuzz/shared/message_db/message_db.dart';
import 'package:kuzz/shared/message_db/message_store.dart';
import 'package:kuzz/shared/relay/relay.dart';

const _channelId = '11111111-1111-4111-8111-111111111111';
const _rootId = 'root';
const _scope = MessageScope(relayUrl: 'https://relay.test', pubkey: 'me');
const _thread = ThreadRepliesArgs(channelId: _channelId, rootId: _rootId);

/// A thread opens from the message database: the first build returns its
/// stored replies, and the relay is asked only for what is not stored.
void main() {
  late Directory root;
  late MessageStore messages;
  late _Session session;
  late ProviderContainer container;

  setUp(() {
    root = Directory.systemTemp.createTempSync('thread_replies_store_test');
    messages = MessageStore(
      () async => '${root.path}/messages',
      rootPath: '${root.path}/messages',
    );
    session = _Session();
    container = ProviderContainer(
      overrides: [
        relaySessionProvider.overrideWith(() => session),
        messageScopeProvider.overrideWithValue(_scope),
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
      _scope,
      MessageDbWrite(
        rows: messageRowsFor(events, receivedAt: 1),
        claims: [
          for (final (lo, hi) in ranges)
            CoverageClaim(
              channelId: _channelId,
              stream: CoverageStream.thread(_rootId),
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

  List<String> replyIds(List<NostrEvent> events) => [
    for (final event in events)
      if (event.kind == EventKind.streamMessage) event.id,
  ];

  CoverageCursor at(int createdAt, [String id = '']) =>
      CoverageCursor(createdAt, id);

  test('a covered thread opens synchronously with no request', () async {
    await seed(
      [_reply('r1', 10), _reply('r2', 20), _reaction('like', 21, 'r2')],
      ranges: [(CoverageCursor.start, at(20, 'r2'))],
      liveFrom: 20,
    );
    feedRoom();

    final value = container.read(threadRepliesProvider(_thread));
    expect(value, isA<AsyncData<List<NostrEvent>>>());
    expect(replyIds(value.value!), ['r1', 'r2']);
    expect([for (final e in value.value!) e.id], contains('like'));
    final window = container.read(threadWindowProvider(_thread));
    expect(window!.state.hasOlder, isFalse);

    await _pump();
    expect(session.queryFilters, isEmpty);
  });

  test('a thread not proven up to now shows loading and asks', () async {
    await seed(
      [_reply('r1', 10), _reply('r2', 20)],
      ranges: [(CoverageCursor.start, at(20, 'r2'))],
    );
    feedRoom();
    session.queryResults.add([_reply('r2', 20), _reply('r1', 10), _bounds()]);

    final sub = container.listen(threadRepliesProvider(_thread), (_, _) {});
    expect(sub.read(), isA<AsyncLoading<List<NostrEvent>>>());
    final replies = await container.read(threadRepliesProvider(_thread).future);
    expect(replyIds(replies), ['r1', 'r2']);
    expect(session.queryFilters, isNotEmpty);
    sub.close();
  });

  test('an unfed room does not count as covered', () async {
    await seed(
      [_reply('r1', 10)],
      ranges: [(CoverageCursor.start, at(10, 'r1'))],
      liveFrom: 10,
    );
    final value = container.read(threadRepliesProvider(_thread));
    expect(value, isA<AsyncLoading<List<NostrEvent>>>());
  });

  test('older pages read from the database', () async {
    final replies = [for (var i = 0; i < 60; i++) _reply('r$i', 100 + i)];
    await seed(
      replies,
      ranges: [(CoverageCursor.start, at(159, 'r59'))],
      liveFrom: 159,
    );
    feedRoom();

    final first = container.read(threadRepliesProvider(_thread)).value!;
    expect(replyIds(first), hasLength(50));
    final notifier = container.read(threadWindowProvider(_thread).notifier);
    expect(notifier.hasOlder, isTrue);

    expect(await notifier.loadOlder(), isTrue);
    final window = container.read(threadWindowProvider(_thread))!;
    expect(replyIds(window.events), hasLength(60));
    expect(window.state.hasOlder, isFalse);
    expect(session.queryFilters, isEmpty);
  });

  test(
    'a newest page fetched while fed lets the next open skip the relay',
    () async {
      await seed(const [], liveFrom: 30);
      feedRoom();
      session.queryResults.add([_reply('r2', 20), _reply('r1', 10), _bounds()]);

      final sub = container.listen(threadRepliesProvider(_thread), (_, _) {});
      await container.read(threadRepliesProvider(_thread).future);
      final asked = session.queryFilters.length;
      expect(asked, greaterThan(0));
      sub.close();
      await messages.settled();

      // The page joined the live range: a fresh open needs no request.
      final reopenedSession = _Session();
      final reopened = ProviderContainer(
        overrides: [
          relaySessionProvider.overrideWith(() => reopenedSession),
          messageScopeProvider.overrideWithValue(_scope),
          messageStoreProvider.overrideWithValue(messages),
        ],
      );
      addTearDown(reopened.dispose);
      reopened.read<ChannelRoomStore>(channelRoomStoreProvider)
        ..markLive()
        ..setFedRooms(_scope, {_channelId});
      final value = reopened.read(threadRepliesProvider(_thread));
      expect(value, isA<AsyncData<List<NostrEvent>>>());
      expect(replyIds(value.value!), ['r1', 'r2']);
      await _pump();
      expect(reopenedSession.queryFilters, isEmpty);
    },
  );
}

NostrEvent _reply(String id, int createdAt) => NostrEvent(
  id: id,
  pubkey: 'alice',
  createdAt: createdAt,
  kind: EventKind.streamMessage,
  tags: const [
    ['h', _channelId],
    ['e', _rootId, '', 'root'],
    ['e', _rootId, '', 'reply'],
  ],
  content: id,
  sig: 'sig',
);

NostrEvent _reaction(String id, int createdAt, String target) => NostrEvent(
  id: id,
  pubkey: 'alice',
  createdAt: createdAt,
  kind: EventKind.reaction,
  tags: [
    const ['h', _channelId],
    ['e', target],
  ],
  content: '+',
  sig: 'sig',
);

NostrEvent _bounds({bool hasMore = false}) => NostrEvent(
  id: 'thread-bounds',
  pubkey: 'relay',
  createdAt: 0,
  kind: threadWindowBoundsKind,
  tags: const [
    ['h', _channelId],
    ['e', _rootId],
  ],
  content: jsonEncode({'has_more': hasMore, 'next_cursor': null}),
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
  }) async => const [];

  @override
  Future<void Function()> subscribe(
    NostrFilter filter,
    void Function(NostrEvent) onEvent, {
    void Function(String message)? onClosed,
  }) async => () {};
}
