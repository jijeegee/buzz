import 'dart:convert';
import 'dart:io';

import 'package:kuzz/features/channels/channel_head_cache.dart';
import 'package:kuzz/features/channels/channel_preload.dart';
import 'package:kuzz/features/channels/thread_replies_provider.dart';
import 'package:kuzz/shared/relay/relay.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

const _scope = ChannelHeadScope(
  relayUrl: 'https://relay.example',
  pubkey: 'me',
);

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

    setUp(() async {
      root = await Directory.systemTemp.createTemp('channel_preload_test');
      session = _WindowSession();
      now = DateTime(2026, 1, 1, 12);
      container = ProviderContainer(
        overrides: [
          relaySessionProvider.overrideWith(() => session),
          channelHeadScopeProvider.overrideWithValue(_scope),
          channelHeadCacheProvider.overrideWithValue(
            ChannelHeadCache(() async => root),
          ),
          channelPreloaderProvider.overrideWith(
            (ref) => ChannelPreloader(ref, now: () => now),
          ),
        ],
      );
    });

    tearDown(() async {
      container.dispose();
      if (await root.exists()) await root.delete(recursive: true);
    });

    test(
      'loads up to twelve rooms in one request and stores each head',
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
        final head = await container
            .read(channelHeadCacheProvider)
            .load(_scope, 'room-5');
        expect(head?.map((event) => event.id), [
          'room-5-message',
          'room-5-reaction',
          'room-5-bounds',
        ]);
      },
    );

    test('skips fresh heads and rooms tried within five minutes', () async {
      final preloader = container.read(channelPreloaderProvider);
      await container.read(channelHeadCacheProvider).store(_scope, 'fresh', [
        _event('cached', 9),
      ]);

      await preloader.preloadRooms([
        (channelId: 'fresh', latestAt: 1),
        (channelId: 'cold', latestAt: 1),
      ]);
      expect(session.requests.single.map((f) => f.tags['#h']!.single), [
        'cold',
      ]);

      // A head older than the room's newest message is reloaded.
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
}
