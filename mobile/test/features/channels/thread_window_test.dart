import 'dart:convert';

import 'package:buzz/features/channels/thread_replies_provider.dart';
import 'package:buzz/features/channels/thread_window.dart';
import 'package:buzz/shared/relay/relay.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

const _room = 'room';
const _root = 'root';

/// Reply `n` is created at second `n`; a higher n is newer.
NostrEvent _reply(int n, {int kind = EventKind.streamMessage}) => NostrEvent(
  id: n.toRadixString(16).padLeft(64, '0'),
  pubkey: 'b' * 64,
  createdAt: n,
  kind: kind,
  tags: const [
    ['h', _room],
    ['e', _root, '', 'root'],
  ],
  content: 'r$n',
  sig: '',
);

ThreadWindowCursor _cursor(int n) => ThreadWindowCursor.of(_reply(n));

/// A relay page holding replies [from]..[to] (inclusive), newest first.
ThreadWindowPage _page(int from, int to, {required bool hasMore}) =>
    ThreadWindowPage(
      events: [for (var n = to; n >= from; n--) _reply(n)],
      hasMore: hasMore,
      nextCursor: hasMore ? _cursor(from) : null,
    );

ThreadWindow _cached(List<NostrEvent> events, int coveredTo) => ThreadWindow(
  events: events,
  state: ThreadWindowState(hasOlder: false, newestCovered: _cursor(coveredTo)),
);

List<int> _ids(ThreadWindow window) =>
    window.events.map((event) => event.createdAt).toList();

List<int> _range(int from, int to) => [for (var n = from; n <= to; n++) n];

/// The relay's bounds overlay closing one thread window.
NostrEvent _bounds({required bool hasMore, int? next}) => NostrEvent(
  id: 'f' * 64,
  pubkey: 'relay',
  createdAt: 0,
  kind: threadWindowBoundsKind,
  tags: const [
    ['h', _room],
    ['e', _root],
  ],
  content: jsonEncode({
    'has_more': hasMore,
    'next_cursor': next == null
        ? null
        : {'created_at': next, 'id': _reply(next).id},
  }),
  sig: '',
);

/// Serves thread windows over replies 1..[total], newest first.
class _WindowRelay extends RelaySessionNotifier {
  _WindowRelay(this.total);

  int total;
  bool servesWindows = true;
  final windowFilters = <NostrFilter>[];
  var fullScans = 0;

  @override
  SessionState build() => const SessionState(status: SessionStatus.connected);

  @override
  Future<List<NostrEvent>> queryRelay(
    List<NostrFilter> filters, {
    Duration timeout = const Duration(seconds: 8),
  }) async {
    final filter = filters.single;
    if (filter.extensions['thread_window'] == true && !servesWindows) {
      return const [];
    }
    if (filter.extensions['thread_window'] != true) {
      fullScans++;
      return [for (var n = 1; n <= total; n++) _reply(n)];
    }
    windowFilters.add(filter);
    final newest = filter.until == null ? total : filter.until! - 1;
    final oldest = (newest - filter.limit + 1).clamp(1, newest);
    final hasMore = oldest > 1;
    return [
      for (var n = newest; n >= oldest; n--) _reply(n),
      _bounds(hasMore: hasMore, next: hasMore ? oldest : null),
    ];
  }
}

void main() {
  group('mergeNewestThreadPage', () {
    test('a cold thread keeps just the newest page, oldest first', () {
      final window = mergeNewestThreadPage(null, _page(51, 100, hasMore: true));
      expect(_ids(window), _range(51, 100));
      expect(window.state.hasOlder, isTrue);
      expect(window.state.olderCursor, _cursor(51));
      expect(window.state.newestCovered, _cursor(100));
    });

    test(
      'reopening keeps older loaded replies that connect to the new page',
      () {
        final window = mergeNewestThreadPage(
          _cached([for (var n = 1; n <= 60; n++) _reply(n)], 60),
          _page(13, 62, hasMore: true),
        );
        expect(_ids(window), _range(1, 62));
        expect(window.state.hasOlder, isFalse);
        expect(window.state.newestCovered, _cursor(62));
      },
    );

    test(
      'a page that cannot reach the cached replies restarts from the page',
      () {
        final window = mergeNewestThreadPage(
          _cached([for (var n = 1; n <= 10; n++) _reply(n)], 10),
          _page(51, 100, hasMore: true),
        );
        expect(_ids(window), _range(51, 100));
        expect(window.state.hasOlder, isTrue);
      },
    );

    test('a live reply inside the new page does not hide a gap', () {
      final window = mergeNewestThreadPage(
        _cached([for (var n = 1; n <= 50; n++) _reply(n), _reply(111)], 50),
        _page(62, 111, hasMore: true),
      );
      expect(_ids(window), _range(62, 111));
      expect(window.state.olderCursor, _cursor(62));
    });

    test('reactions in the cache are kept, not mistaken for replies', () {
      final reaction = NostrEvent(
        id: 'r' * 64,
        pubkey: 'b' * 64,
        createdAt: 55,
        kind: 7,
        tags: const [],
        content: '+',
        sig: '',
      );
      final window = mergeNewestThreadPage(
        _cached([for (var n = 1; n <= 60; n++) _reply(n), reaction], 60),
        _page(13, 62, hasMore: true),
      );
      expect(window.events.any((event) => event.id == reaction.id), isTrue);
      expect(
        window.events.where((e) => e.kind == EventKind.streamMessage).length,
        62,
      );
    });

    test('replies deleted inside the page span drop out; live ones stay', () {
      final fresh = ThreadWindowPage(
        events: [_reply(7), _reply(5)],
        hasMore: false,
      );
      final window = mergeNewestThreadPage(
        _cached([_reply(5), _reply(6), _reply(7), _reply(9)], 7),
        fresh,
      );
      expect(_ids(window), [5, 7, 9]);
    });
  });

  test('mergeOlderThreadPage prepends and moves the cursor', () {
    final newest = mergeNewestThreadPage(null, _page(51, 100, hasMore: true));
    final window = mergeOlderThreadPage(newest, _page(1, 50, hasMore: false));
    expect(_ids(window), _range(1, 100));
    expect(window.state.hasOlder, isFalse);
    expect(window.state.newestCovered, _cursor(100));
  });

  test('parseThreadWindowPage reads the bounds and returns null without', () {
    final page = parseThreadWindowPage(
      [_reply(3), _reply(2), _bounds(hasMore: true, next: 2)],
      channelId: _room,
      rootId: _root,
    );
    expect(page?.events.map((e) => e.createdAt), [3, 2]);
    expect(page?.hasMore, isTrue);
    expect(page?.nextCursor, _cursor(2));
    expect(
      parseThreadWindowPage([_reply(3)], channelId: _room, rootId: _root),
      isNull,
    );
  });

  group('threadRepliesProvider with thread windows', () {
    const args = ThreadRepliesArgs(channelId: _room, rootId: _root);

    test(
      'opens with the newest 50 and pages older replies on demand',
      () async {
        final relay = _WindowRelay(120);
        final container = ProviderContainer(
          overrides: [relaySessionProvider.overrideWith(() => relay)],
        );
        addTearDown(container.dispose);
        final shown = container.listen(
          threadRepliesWithLocalProvider(args),
          (_, _) {},
        );

        final first = await container.read(threadRepliesProvider(args).future);
        expect(first.map((e) => e.createdAt), _range(71, 120));
        expect(relay.fullScans, 0);
        expect(
          container.read(threadWindowProvider(args).notifier).hasOlder,
          isTrue,
        );

        final window = container.read(threadWindowProvider(args).notifier);
        expect(await window.loadOlder(), isTrue);
        expect(await window.loadOlder(), isTrue);
        expect(window.hasOlder, isFalse);
        await container.pump();
        expect(shown.read().value?.map((e) => e.createdAt), _range(1, 120));
        // Paging folds into the window without refetching the newest page.
        expect(relay.windowFilters.length, 3);
        expect(relay.windowFilters[1].until, 71);
        expect(relay.windowFilters[1].extensions['before_id'], _reply(71).id);
      },
    );

    test('a complete scan stays connected to the next newest page', () async {
      final relay = _WindowRelay(120)..servesWindows = false;
      final container = ProviderContainer(
        overrides: [relaySessionProvider.overrideWith(() => relay)],
      );
      addTearDown(container.dispose);
      final shown = container.listen(
        threadRepliesWithLocalProvider(args),
        (_, _) {},
      );
      await container.read(threadRepliesProvider(args).future);
      expect(relay.fullScans, 1);

      relay
        ..servesWindows = true
        ..total = 121;
      container.invalidate(threadRepliesProvider(args));
      await container.read(threadRepliesProvider(args).future);
      await container.pump();
      expect(shown.read().value?.map((e) => e.createdAt), _range(1, 121));
      expect(
        container.read(threadWindowProvider(args).notifier).hasOlder,
        isFalse,
      );
    });

    test('reopening folds one newest page into the held replies', () async {
      final relay = _WindowRelay(120);
      final container = ProviderContainer(
        overrides: [relaySessionProvider.overrideWith(() => relay)],
      );
      addTearDown(container.dispose);
      final shown = container.listen(
        threadRepliesWithLocalProvider(args),
        (_, _) {},
      );
      await container.read(threadRepliesProvider(args).future);
      await container.read(threadWindowProvider(args).notifier).loadOlder();

      relay.total = 122;
      container.invalidate(threadRepliesProvider(args));
      await container.read(threadRepliesProvider(args).future);
      await container.pump();
      expect(shown.read().value?.map((e) => e.createdAt), _range(21, 122));
      expect(relay.fullScans, 0);
    });
  });
}
