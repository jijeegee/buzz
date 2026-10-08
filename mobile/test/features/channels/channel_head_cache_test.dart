import 'dart:async';
import 'dart:collection';
import 'dart:convert';
import 'dart:io';

import 'package:buzz/features/channels/channel_head_cache.dart';
import 'package:buzz/features/channels/channel_messages_provider.dart';
import 'package:buzz/shared/relay/relay.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

/// Persisted channel heads paint a channel before the relay answers, and the
/// relay's newest page always replaces them.
void main() {
  group('ChannelHeadCache', () {
    late Directory root;
    late ChannelHeadCache cache;

    setUp(() async {
      root = await Directory.systemTemp.createTemp('channel_heads_test');
      cache = ChannelHeadCache(() async => root);
    });

    tearDown(() async {
      if (await root.exists()) await root.delete(recursive: true);
    });

    test('round-trips a head per account and relay', () async {
      await cache.store(_scope, _channelId, [_event(id: 'a', createdAt: 1)]);

      final loaded = await cache.load(_scope, _channelId);
      expect(loaded?.map((event) => event.toJson()), [
        _event(id: 'a', createdAt: 1).toJson(),
      ]);
      expect(await cache.load(_otherAccount, _channelId), isNull);
      expect(await cache.load(_scope, 'other-channel'), isNull);
    });

    test('keeps only the most recently used channels', () async {
      final small = ChannelHeadCache(() async => root, maxChannels: 3);
      for (var i = 0; i < 3; i++) {
        await small.store(_scope, 'channel-$i', [_event(id: 'e$i')]);
      }
      // Order the entries deterministically: channel-0 is least recent.
      final base = DateTime(2020);
      for (var i = 0; i < 3; i++) {
        _entryFiles(root)
            .singleWhere(
              (file) => file.readAsStringSync().contains('"channel-$i"'),
            )
            .setLastModifiedSync(base.add(Duration(minutes: i)));
      }
      // Reading a head marks it as recently used.
      expect(await small.load(_scope, 'channel-0'), isNotNull);

      await small.store(_scope, 'channel-new', [_event(id: 'new')]);

      expect(_entryFiles(root), hasLength(3));
      expect(await small.load(_scope, 'channel-1'), isNull);
      expect(await small.load(_scope, 'channel-0'), isNotNull);
      expect(await small.load(_scope, 'channel-2'), isNotNull);
      expect(await small.load(_scope, 'channel-new'), isNotNull);
    });

    test('drops an oversized head instead of keeping a stale one', () async {
      await cache.store(_scope, _channelId, [_event(id: 'small')]);
      final huge = 'x' * (ChannelHeadCache.maxEntryBytes + 1);
      await cache.store(_scope, _channelId, [_event(id: 'big', content: huge)]);

      expect(await cache.load(_scope, _channelId), isNull);
    });

    test('treats a corrupt entry as a miss and removes it', () async {
      await cache.store(_scope, _channelId, [_event(id: 'a')]);
      final file = _entryFiles(root).single..writeAsStringSync('{"v":1,');

      expect(await cache.load(_scope, _channelId), isNull);
      expect(file.existsSync(), isFalse);
    });

    test('a clear drops writes that were requested before it', () async {
      final write = cache.store(_scope, _channelId, [_event(id: 'a')]);
      final clear = cache.clearAll();
      await Future.wait([write, clear]);

      expect(await cache.load(_scope, _channelId), isNull);
      expect(root.existsSync(), isFalse);
    });

    test('clears one relay or everything', () async {
      const otherRelay = ChannelHeadScope(
        relayUrl: 'https://other.example',
        pubkey: 'alice',
      );
      await cache.store(_scope, _channelId, [_event(id: 'a')]);
      await cache.store(_otherAccount, _channelId, [_event(id: 'b')]);
      await cache.store(otherRelay, _channelId, [_event(id: 'c')]);

      await cache.clearRelay(_scope.relayUrl);
      expect(await cache.load(_scope, _channelId), isNull);
      expect(await cache.load(_otherAccount, _channelId), isNull);
      expect(await cache.load(otherRelay, _channelId), isNotNull);

      await cache.clearAll();
      expect(await cache.load(otherRelay, _channelId), isNull);
    });
  });

  group('ChannelMessagesNotifier with a cached head', () {
    test('paints the cached head, then the relay page replaces it', () async {
      final page = Completer<List<NostrEvent>>();
      final cache = _MemoryHeadCache({
        _channelId: [
          _event(id: 'deleted-elsewhere', createdAt: 20),
          _event(id: 'kept', createdAt: 10),
          _bounds(),
        ],
      });
      final session = _Session(queryResults: [page.future]);
      final container = _container(session, cache);
      addTearDown(container.dispose);

      container.read(channelMessagesProvider(_channelId));
      await _pump();
      expect(_ids(container), ['kept', 'deleted-elsewhere']);

      page.complete([
        _event(id: 'new', createdAt: 30),
        _event(id: 'kept', createdAt: 10),
        _bounds(),
      ]);
      await _pump();
      expect(_ids(container), ['kept', 'new']);
      expect(cache.stored[_channelId]?.map((event) => event.id), [
        'new',
        'kept',
        _bounds().id,
      ]);
    });

    test(
      'cached rows cannot be paged or used for unread until the relay page',
      () async {
        final page = Completer<List<NostrEvent>>();
        final cache = _MemoryHeadCache({
          _channelId: [
            _event(id: 'cached', createdAt: 10),
            _bounds(hasMore: true),
          ],
        });
        final session = _Session(queryResults: [page.future]);
        final container = _container(session, cache);
        addTearDown(container.dispose);

        container.read(channelMessagesProvider(_channelId));
        await _pump();
        final notifier = container.read(
          channelMessagesProvider(_channelId).notifier,
        );
        expect(notifier.isShowingCachedHead, isTrue);
        expect(await notifier.fetchOlder(), isFalse);
        expect(session.queryCount, 1);

        page.complete([_event(id: 'fresh', createdAt: 20), _bounds()]);
        await _pump();
        expect(notifier.isShowingCachedHead, isFalse);
      },
    );

    test('keeps live events that arrive while the cached head shows', () async {
      final page = Completer<List<NostrEvent>>();
      final cache = _MemoryHeadCache({
        _channelId: [_event(id: 'cached', createdAt: 10), _bounds()],
      });
      final session = _Session(queryResults: [page.future]);
      final container = _container(session, cache);
      addTearDown(container.dispose);

      container.read(channelMessagesProvider(_channelId));
      await _pump();
      session.emit(_event(id: 'live', createdAt: 40));
      expect(_ids(container), ['cached', 'live']);

      page.complete([_event(id: 'history', createdAt: 20), _bounds()]);
      await _pump();
      expect(_ids(container), ['history', 'live']);
    });

    test('ignores a cached head that loads after the relay page', () async {
      final load = Completer<List<NostrEvent>?>();
      final cache = _MemoryHeadCache({}, loadGate: load);
      final session = _Session(
        queryResults: [
          [_event(id: 'fresh', createdAt: 20), _bounds()],
        ],
      );
      final container = _container(session, cache);
      addTearDown(container.dispose);

      container.read(channelMessagesProvider(_channelId));
      await _pump();
      expect(_ids(container), ['fresh']);

      load.complete([_event(id: 'stale', createdAt: 10), _bounds()]);
      await _pump();
      expect(_ids(container), ['fresh']);
    });

    test('shows the cached head while offline', () async {
      final cache = _MemoryHeadCache({
        _channelId: [_event(id: 'cached', createdAt: 10), _bounds()],
      });
      final session = _Session(status: SessionStatus.disconnected);
      final container = _container(session, cache);
      addTearDown(container.dispose);

      container.read(channelMessagesProvider(_channelId));
      await _pump();
      expect(_ids(container), ['cached']);
      expect(
        container
            .read(channelMessagesProvider(_channelId).notifier)
            .hasLoadedMessages,
        isTrue,
      );
    });

    test('does not persist a websocket fallback history', () async {
      final cache = _MemoryHeadCache({});
      final session = _Session(
        historyResults: [
          [_event(id: 'legacy', createdAt: 10)],
        ],
      );
      final container = _container(session, cache);
      addTearDown(container.dispose);

      container.read(channelMessagesProvider(_channelId));
      await _pump();
      expect(_ids(container), ['legacy']);
      expect(cache.stored, isEmpty);
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

List<File> _entryFiles(Directory root) => root
    .listSync(recursive: true)
    .whereType<File>()
    .where((file) => file.path.endsWith('.json'))
    .toList();

ProviderContainer _container(_Session session, ChannelHeadCache cache) =>
    ProviderContainer(
      overrides: [
        relaySessionProvider.overrideWith(() => session),
        channelHeadCacheProvider.overrideWithValue(cache),
        channelHeadScopeProvider.overrideWithValue(_scope),
      ],
    );

List<String>? _ids(ProviderContainer container) => container
    .read(channelMessagesProvider(_channelId))
    .value
    ?.map((event) => event.id)
    .toList();

Future<void> _pump() async {
  for (var i = 0; i < 6; i++) {
    await Future<void>.delayed(Duration.zero);
  }
}

NostrEvent _event({required String id, int createdAt = 1, String? content}) =>
    NostrEvent(
      id: id,
      pubkey: 'alice',
      createdAt: createdAt,
      kind: EventKind.streamMessageV2,
      tags: const [
        ['h', _channelId],
      ],
      content: content ?? id,
      sig: 'sig',
    );

NostrEvent _bounds({bool hasMore = false}) => NostrEvent(
  id: 'bounds',
  pubkey: 'relay',
  createdAt: 0,
  kind: EventKind.channelWindowBounds,
  tags: [
    ['d', '${_channelId.toLowerCase()}:head'],
  ],
  content: jsonEncode({
    'has_more': hasMore,
    'next_cursor': hasMore ? {'created_at': 10, 'id': 'cached'} : null,
  }),
  sig: 'sig',
);

class _MemoryHeadCache extends ChannelHeadCache {
  _MemoryHeadCache(this.entries, {this.loadGate})
    : super(() async => throw UnsupportedError('memory cache'));

  final Map<String, List<NostrEvent>> entries;
  final Completer<List<NostrEvent>?>? loadGate;
  final stored = <String, List<NostrEvent>>{};

  @override
  Future<List<NostrEvent>?> load(ChannelHeadScope scope, String channelId) {
    final gate = loadGate;
    if (gate != null) return gate.future;
    return Future.value(entries[channelId]);
  }

  @override
  Future<void> store(
    ChannelHeadScope scope,
    String channelId,
    List<NostrEvent> events,
  ) async {
    stored[channelId] = events;
  }
}

class _Session extends RelaySessionNotifier {
  _Session({
    this.status = SessionStatus.connected,
    List<Object> queryResults = const [],
    List<List<NostrEvent>> historyResults = const [],
  }) : _queryResults = Queue.of(queryResults),
       _historyResults = Queue.of(historyResults);

  final SessionStatus status;
  final Queue<Object> _queryResults;
  final Queue<List<NostrEvent>> _historyResults;
  final _listeners = <void Function(NostrEvent)>[];
  int queryCount = 0;

  @override
  SessionState build() => SessionState(status: status);

  @override
  Future<List<NostrEvent>> queryRelay(
    List<NostrFilter> filters, {
    Duration timeout = const Duration(seconds: 8),
  }) async {
    queryCount++;
    if (_queryResults.isEmpty) throw Exception('unsupported');
    final result = _queryResults.removeFirst();
    if (result is Future<List<NostrEvent>>) return await result;
    return (result as List<NostrEvent>).toList();
  }

  @override
  Future<List<NostrEvent>> fetchHistory(
    NostrFilter filter, {
    Duration timeout = const Duration(seconds: 8),
  }) async =>
      _historyResults.isEmpty ? const [] : _historyResults.removeFirst();

  @override
  Future<void Function()> subscribe(
    NostrFilter filter,
    void Function(NostrEvent) onEvent, {
    void Function(String message)? onClosed,
  }) async {
    _listeners.add(onEvent);
    return () => _listeners.remove(onEvent);
  }

  void emit(NostrEvent event) {
    for (final listener in List.of(_listeners)) {
      listener(event);
    }
  }
}
