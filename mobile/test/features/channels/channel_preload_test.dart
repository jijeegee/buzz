import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:kuzz/features/channels/channel.dart';
import 'package:kuzz/features/channels/channel_preload.dart';
import 'package:kuzz/features/channels/channel_room_store.dart';
import 'package:kuzz/features/channels/channels_provider.dart';
import 'package:kuzz/features/channels/unread_badge/observed_unread_event.dart';
import 'package:kuzz/features/channels/window_anchor.dart';
import 'package:kuzz/shared/read_state/read_state_provider.dart';
import 'package:kuzz/features/channels/thread_replies_provider.dart';
import 'package:kuzz/shared/message_db/coverage.dart';
import 'package:kuzz/shared/message_db/event_rows.dart';
import 'package:kuzz/shared/message_db/message_db.dart';
import 'package:kuzz/shared/message_db/message_store.dart';
import 'package:kuzz/shared/relay/relay.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import 'window_relay_fixture.dart';

const _scope = MessageScope(relayUrl: 'https://relay.example', pubkey: 'me');

const _messageScope = MessageScope(
  relayUrl: 'https://relay.example',
  pubkey: 'me',
);

String _dbPath(Directory root) => '${root.path}/${_messageScope.relativePath}';

/// Answers batched window requests the way the relay does: each requested
/// room or thread in order, closed by its bounds overlay.
class _WindowSession extends RelaySessionNotifier {
  final List<List<NostrFilter>> requests = [];

  /// Rooms and threads the reader cannot see emit nothing.
  final Set<String> hidden = {};

  @override
  SessionState build() => const SessionState(status: SessionStatus.connected);

  @override
  Future<List<NostrEvent>> queryRelay(
    List<NostrFilter> filters, {
    Duration timeout = const Duration(seconds: 8),
  }) async {
    requests.add(filters);
    return [
      for (final filter in filters)
        if (!hidden.contains(filter.tags['#h']!.single))
          if (filter.extensions['thread_window'] == true)
            ..._threadWindow(
              filter.tags['#h']!.single,
              filter.tags['#e']!.single,
            )
          else
            ..._roomWindow(filter.tags['#h']!.single),
    ];
  }
}

NostrEvent _event(
  String id,
  int kind, {
  List<List<String>> tags = const [],
  String content = '',
  int createdAt = 100,
}) => NostrEvent(
  id: id,
  pubkey: 'other',
  createdAt: createdAt,
  kind: kind,
  tags: tags,
  content: content,
  sig: '',
);

List<NostrEvent> _roomWindow(String channelId) => [
  _event(
    '$channelId-message',
    9,
    tags: [
      ['h', channelId],
    ],
  ),
  // An aux event without the room tag still belongs to the open window.
  _event('$channelId-reaction', 7),
  _event(
    '$channelId-bounds',
    EventKind.channelWindowBounds,
    tags: [
      ['d', '$channelId:head'],
      ['h', channelId],
    ],
    content: jsonEncode({'has_more': false, 'next_cursor': null}),
  ),
];

List<NostrEvent> _threadWindow(String channelId, String rootId) => [
  _event(
    '$rootId-reply',
    9,
    tags: [
      ['h', channelId],
      ['e', rootId, '', 'root'],
      ['e', rootId, '', 'reply'],
    ],
  ),
  _event(
    '$rootId-bounds',
    39007,
    tags: [
      ['h', channelId],
      ['e', rootId],
    ],
    content: jsonEncode({'has_more': true, 'next_cursor': null}),
  ),
];

void main() {
  group('splitting batched windows', () {
    test('each room window ends at its bounds overlay', () {
      final windows = splitChannelWindows(
        [
          ..._roomWindow('a'),
          ..._roomWindow('b'),
          ..._roomWindow('x'),
          _event('trailing', 9),
        ],
        ['a', 'b'],
      );
      expect(windows.keys, ['a', 'b']);
      expect(windows['b']!.map((event) => event.id), [
        'b-message',
        'b-reaction',
        'b-bounds',
      ]);
    });

    test('each thread window ends at its bounds overlay', () {
      const wanted = ThreadRepliesArgs(channelId: 'a', rootId: 'r1');
      final windows = splitThreadWindows(
        [..._threadWindow('a', 'r1'), ..._threadWindow('a', 'r2')],
        [wanted],
      );
      expect(windows.keys, [wanted]);
      expect(windows[wanted]!.map((event) => event.id), ['r1-reply']);
    });
  });

  group('ChannelPreloader', () {
    late Directory root;
    late _WindowSession session;
    late ProviderContainer container;
    late DateTime now;
    late Directory messagesRoot;
    late MessageStore messages;

    setUp(() async {
      root = await Directory.systemTemp.createTemp('channel_preload_test');
      messagesRoot = await Directory.systemTemp.createTemp('preload_messages');
      messages = MessageStore(
        () async => messagesRoot.path,
        rootPath: messagesRoot.path,
      );
      session = _WindowSession();
      now = DateTime(2026, 1, 1, 12);
      container = ProviderContainer(
        overrides: [
          relaySessionProvider.overrideWith(() => session),
          messageScopeProvider.overrideWithValue(_scope),
          messageStoreProvider.overrideWithValue(messages),
          channelPreloaderProvider.overrideWith(
            (ref) => ChannelPreloader(ref, now: () => now),
          ),
        ],
      );
    });

    tearDown(() async {
      container.dispose();
      await messages.close();
      if (await root.exists()) await root.delete(recursive: true);
      await messagesRoot.delete(recursive: true);
    });

    test(
      'stores preloaded rooms and threads in the message database',
      () async {
        final preloader = container.read(channelPreloaderProvider);
        await preloader.preloadRooms([(channelId: 'room-1', latestAt: 100)]);
        await preloader.preloadThreads([
          const ThreadRepliesArgs(channelId: 'room-1', rootId: 'root'),
        ]);
        await messages.settled();

        final db = openMessageDbReader(_dbPath(messagesRoot));
        addTearDown(db.close);
        expect(
          db
              .select('SELECT id, channel_id FROM events ORDER BY id')
              .map((row) => (row['id'], row['channel_id'])),
          [
            ('room-1-message', 'room-1'),
            // The aux event has no room tag; its window names the room.
            ('room-1-reaction', 'room-1'),
            ('root-reply', 'room-1'),
          ],
        );
        expect(
          db
              .select(
                'SELECT stream, lo_at, hi_at, hi_id FROM coverage '
                'ORDER BY stream',
              )
              .map(
                (row) =>
                    (row['stream'], row['lo_at'], row['hi_at'], row['hi_id']),
              ),
          [
            // has_more false: the room's whole history is this page.
            ('', -1, 100, 'room-1-message'),
            // A preloaded thread page proves only the span of its rows.
            ('root:window', 100, 100, 'root-reply'),
          ],
        );
      },
    );

    test(
      'loads up to twelve rooms in one request and stores each page',
      () async {
        final preloader = container.read(channelPreloaderProvider);
        session.hidden.add('room-3');
        final stored = await preloader.preloadRooms([
          for (var i = 0; i < 14; i++) (channelId: 'room-$i', latestAt: 100),
        ]);

        expect(session.requests, hasLength(1));
        expect(session.requests.single, hasLength(maxChannelWindowsPerRequest));
        final filter = session.requests.single.first;
        expect(filter.limit, channelPreloadMessages);
        expect(filter.extensions['top_level'], isTrue);
        expect(stored, hasLength(11));
        expect(stored, isNot(contains('room-3')));
        await messages.settled();
        final db = openMessageDbReader(_dbPath(messagesRoot));
        addTearDown(db.close);
        expect(
          db
              .select(
                "SELECT id FROM events WHERE channel_id = 'room-5' ORDER BY id",
              )
              .map((row) => row['id']),
          ['room-5-message', 'room-5-reaction'],
        );
      },
    );

    test('skips rooms stored through their newest message and rooms tried '
        'within five minutes', () async {
      final preloader = container.read(channelPreloaderProvider);
      await messages.write(
        _messageScope,
        MessageDbWrite(
          rows: messageRowsFor([
            _event(
              'cached',
              9,
              tags: [
                ['h', 'fresh'],
              ],
              createdAt: 1,
            ),
          ], receivedAt: 1),
          claims: const [
            CoverageClaim(
              channelId: 'fresh',
              stream: CoverageStream.main,
              range: CoverageRange(
                CoverageCursor.start,
                CoverageCursor(1, 'cached'),
              ),
            ),
          ],
        ),
      );

      await preloader.preloadRooms([
        (channelId: 'fresh', latestAt: 1),
        (channelId: 'cold', latestAt: 1),
      ]);
      expect(session.requests.single.map((f) => f.tags['#h']!.single), [
        'cold',
      ]);

      // A room stored only up to before its newest message is reloaded.
      final future = DateTime.now().millisecondsSinceEpoch ~/ 1000 + 3600;
      await preloader.preloadRooms([
        (channelId: 'fresh', latestAt: future),
        (channelId: 'cold', latestAt: future),
      ]);
      expect(session.requests.last.map((f) => f.tags['#h']!.single), ['fresh']);

      now = now.add(preloadRetryAfter);
      await preloader.preloadRooms([(channelId: 'cold', latestAt: future)]);
      expect(session.requests, hasLength(3));
    });

    test('loads threads four per request into the preload store', () async {
      final preloader = container.read(channelPreloaderProvider);
      final threads = [
        for (var i = 0; i < 6; i++)
          ThreadRepliesArgs(channelId: 'room', rootId: 'root-$i'),
      ];
      var requests = 0;
      final loaded = await preloader.preloadThreads(
        threads,
        beforeRequest: () async => ++requests <= 2,
      );

      expect(loaded, 6);
      expect(session.requests.map((filters) => filters.length), [4, 2]);
      expect(session.requests.first.first.extensions['thread_window'], isTrue);
      expect(
        container
            .read(preloadedThreadRepliesProvider(threads[5]))
            .map((event) => event.id),
        ['root-5-reply'],
      );

      // Already tried: nothing is requested again.
      await preloader.preloadThreads(threads);
      expect(session.requests, hasLength(2));
    });

    test('a stopped preload sends no further thread requests', () async {
      final preloader = container.read(channelPreloaderProvider);
      final loaded = await preloader.preloadThreads([
        const ThreadRepliesArgs(channelId: 'room', rootId: 'root'),
      ], beforeRequest: () async => false);
      expect(loaded, 0);
      expect(session.requests, isEmpty);
    });
  });

  test('preloaded replies expire with the thread retention', () {
    var now = DateTime(2026);
    final container = ProviderContainer(
      overrides: [
        preloadedThreadsProvider.overrideWith(
          () => PreloadedThreadsNotifier(now: () => now),
        ),
      ],
    );
    addTearDown(container.dispose);
    const thread = ThreadRepliesArgs(channelId: 'room', rootId: 'root');
    container.read(preloadedThreadsProvider.notifier).seed(thread, [
      _event('reply', 9),
    ]);
    expect(
      container.read(preloadedThreadsProvider.notifier).repliesOf(thread),
      hasLength(1),
    );
    now = now.add(threadRepliesRetention);
    expect(
      container.read(preloadedThreadsProvider.notifier).repliesOf(thread),
      isEmpty,
    );
  });

  group('read marker windows', () {
    const roomId = 'marker-room';
    final rows = [
      for (var n = 1; n <= 80; n++) fixtureRow(roomId, n, 1000 + n),
    ];
    late Directory messagesRoot;
    late MessageStore messages;
    late _MarkerSession session;
    late ProviderContainer container;

    setUp(() async {
      messagesRoot = await Directory.systemTemp.createTemp('marker_messages');
      messages = MessageStore(
        () async => messagesRoot.path,
        rootPath: messagesRoot.path,
      );
      session = _MarkerSession(roomId, rows);
      container = ProviderContainer(
        overrides: [
          relaySessionProvider.overrideWith(() => session),
          messageScopeProvider.overrideWithValue(_scope),
          messageStoreProvider.overrideWithValue(messages),
          channelsProvider.overrideWith(
            () => _UnreadChannels({
              roomId: [_unread(rows[60])],
              // Read already: nothing to load.
              'read-room': [],
            }),
          ),
          readStateProvider.overrideWith(
            () => _MarkerReadState({roomId: 1050, 'read-room': 2000}),
          ),
        ],
      );
    });

    tearDown(() async {
      container.dispose();
      await messages.close();
      await messagesRoot.delete(recursive: true);
    });

    test('stores the pages on both sides of the read marker', () async {
      final preloader = container.read(channelPreloaderProvider);
      expect(await preloader.preloadReadMarkers(), 1);
      expect(session.requests, hasLength(1));
      final filters = session.requests.single;
      expect(filters, hasLength(2));
      expect(filters[0].until, 1050);
      expect(filters[0].extensions['before_id'], zeroEventId);
      expect(filters[1].since, 1050);
      expect(filters[1].extensions['after_id'], zeroEventId);
      await messages.settled();

      final store = container.read(channelRoomStoreProvider);
      final at = CoverageCursor(1050, zeroEventId);
      final older = store.readOlder(_scope, roomId, at);
      final newer = store.readNewer(_scope, roomId, at);
      expect(older?.rows.first.createdAt, 1050);
      expect(newer?.rows.first.createdAt, 1051);

      // Stored now, and tried: no second request.
      expect(await preloader.preloadReadMarkers(), 0);
      expect(session.requests, hasLength(1));
    });

    test('waits for its turn and stops when told to', () async {
      final preloader = container.read(channelPreloaderProvider);
      expect(
        await preloader.preloadReadMarkers(beforeRequest: () async => false),
        0,
      );
      expect(session.requests, isEmpty);
    });
  });
}

ObservedUnreadEvent _unread(NostrEvent event) => makeObservedUnreadEvent(
  id: event.id,
  createdAt: event.createdAt,
  rootId: null,
  highPriority: false,
  channelType: 'stream',
  isThreadedReply: false,
);

class _UnreadChannels extends ChannelsNotifier {
  _UnreadChannels(Map<String, List<ObservedUnreadEvent>> unread)
    : _unread = {
        for (final entry in unread.entries)
          entry.key: {for (final event in entry.value) event.id: event},
      };

  final Map<String, Map<String, ObservedUnreadEvent>> _unread;

  @override
  Map<String, Map<String, ObservedUnreadEvent>>
  get observedUnreadEventsByChannel => _unread;

  @override
  Future<List<Channel>> build() => SynchronousFuture(const []);
}

class _MarkerReadState extends ReadStateNotifier {
  _MarkerReadState(this._contexts);

  final Map<String, int> _contexts;

  @override
  ReadStateState build() => ReadStateState(
    isReady: true,
    pubkey: 'me',
    contexts: _contexts,
    version: 0,
  );
}

/// Serves each window filter of one room as the relay would.
class _MarkerSession extends RelaySessionNotifier {
  _MarkerSession(this.channelId, this.rows);

  final String channelId;
  final List<NostrEvent> rows;
  final List<List<NostrFilter>> requests = [];

  @override
  SessionState build() => const SessionState(status: SessionStatus.connected);

  @override
  Future<List<NostrEvent>> queryRelay(
    List<NostrFilter> filters, {
    Duration timeout = const Duration(seconds: 8),
  }) async {
    requests.add(filters);
    return [
      for (final filter in filters)
        ...serveChannelWindow(channelId, rows, filter),
    ];
  }
}
