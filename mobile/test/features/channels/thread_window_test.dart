import 'dart:convert';

import 'package:kuzz/features/channels/thread_replies_provider.dart';
import 'package:kuzz/features/channels/thread_window.dart';
import 'package:kuzz/features/channels/window_anchor.dart';
import 'package:kuzz/shared/relay/relay.dart';
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
NostrEvent _bounds({
  required bool hasMore,
  int? next,
  String direction = 'older',
}) => NostrEvent(
  id: 'f' * 64,
  pubkey: 'relay',
  createdAt: 0,
  kind: threadWindowBoundsKind,
  tags: const [
    ['h', _room],
    ['e', _root],
  ],
  content: jsonEncode({
    'version': 1,
    'direction': direction,
    'has_more': hasMore,
    'next_cursor': next == null
        ? null
        : {'created_at': next, 'id': _reply(next).id},
  }),
  sig: '',
);

/// A relay page holding replies [from]..[to] (inclusive), oldest first.
ThreadWindowPage _newerPage(int from, int to, {required bool hasMore}) =>
    ThreadWindowPage(
      events: [for (var n = from; n <= to; n++) _reply(n)],
      hasMore: hasMore,
      nextCursor: hasMore ? _cursor(to) : null,
      direction: WindowPageDirection.newer,
    );

/// Serves thread windows over replies 1..[total] with the relay's keyset
/// predicates: newest first from `until`/`before_id`, or oldest first after
/// `since`/`after_id`.
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
    final afterId = filter.extensions['after_id'] as String?;
    final beforeId = filter.extensions['before_id'] as String?;
    final all = [for (var n = 1; n <= total; n++) _reply(n)];
    final List<NostrEvent> candidates;
    if (afterId != null) {
      final since = filter.since!;
      candidates = all
          .where(
            (e) =>
                e.createdAt > since ||
                (e.createdAt == since && e.id.compareTo(afterId) < 0),
          )
          .toList();
    } else {
      final until = filter.until;
      candidates = all.reversed
          .where(
            (e) =>
                until == null ||
                e.createdAt < until ||
                (e.createdAt == until && e.id.compareTo(beforeId!) > 0),
          )
          .toList();
    }
    final page = candidates.take(filter.limit).toList();
    final hasMore = candidates.length > filter.limit;
    return [
      ...page,
      _bounds(
        hasMore: hasMore,
        next: hasMore ? page.last.createdAt : null,
        direction: afterId == null ? 'older' : 'newer',
      ),
    ];
  }
}

Future<void> _settle(ProviderContainer container) async {
  for (var i = 0; i < 5; i++) {
    await container.pump();
    await Future<void>.delayed(Duration.zero);
  }
}

ProviderContainer _container(_WindowRelay relay, {bool newer = true}) {
  final container = ProviderContainer(
    overrides: [
      relaySessionProvider.overrideWith(() => relay),
      relayInfoProvider.overrideWith(
        (ref) async =>
            RelayInfo(supportedExtensions: {if (newer) newerWindowExtension}),
      ),
    ],
  );
  addTearDown(container.dispose);
  return container;
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

  group('middle thread window', () {
    // Opened at reply 30: older 11..30, newer 31..50 with more to come.
    ThreadWindow middle() => mergeMiddleThreadPages(
      null,
      _page(11, 30, hasMore: true),
      _newerPage(31, 50, hasMore: true),
    );

    test('opens with a page each way and pages forward without gaps', () {
      var window = middle();
      expect(_ids(window), _range(11, 50));
      expect(window.state.hasOlder, isTrue);
      expect(window.state.hasNewer, isTrue);
      expect(window.state.newerCursor, _cursor(50));
      window = mergeNewerThreadPage(window, _newerPage(51, 70, hasMore: true));
      window = mergeNewerThreadPage(window, _newerPage(71, 80, hasMore: false));
      expect(_ids(window), _range(11, 80));
      expect(window.state.hasNewer, isFalse);
      expect(window.state.newerCursor, isNull);
      expect(window.state.newestCovered, _cursor(80));
      expect(window.state.olderCursor, _cursor(11));
    });

    test('a newest-page refresh does not clobber it before they meet', () {
      final window = middle();
      expect(
        mergeNewestThreadPage(window, _page(71, 120, hasMore: true)),
        same(window),
      );
    });

    test('a newest page that reaches it connects and ends hasNewer', () {
      final window = mergeNewestThreadPage(
        middle(),
        _page(41, 90, hasMore: true),
      );
      expect(_ids(window), _range(11, 90));
      expect(window.state.hasNewer, isFalse);
      expect(window.state.hasOlder, isTrue);
      expect(window.state.olderCursor, _cursor(11));
      expect(window.state.newestCovered, _cursor(90));
    });

    test('older pages keep its newer edge', () {
      final window = mergeOlderThreadPage(
        middle(),
        _page(1, 10, hasMore: false),
      );
      expect(_ids(window), _range(1, 50));
      expect(window.state.hasNewer, isTrue);
      expect(window.state.newerCursor, _cursor(50));
    });

    test('holds live replies back while newer pages remain', () {
      final state = middle().state;
      expect(isCoveredByThreadWindow(_reply(500), state), isTrue);
      final reaction = NostrEvent(
        id: 'r' * 64,
        pubkey: 'b' * 64,
        createdAt: 500,
        kind: 7,
        tags: const [],
        content: '+',
        sig: '',
      );
      expect(isCoveredByThreadWindow(reaction, state), isFalse);
      final connected = mergeNewerThreadPage(
        middle(),
        _newerPage(51, 60, hasMore: false),
      ).state;
      expect(isCoveredByThreadWindow(_reply(500), connected), isFalse);
      expect(isCoveredByThreadWindow(_reply(55), connected), isTrue);
    });

    test('parseThreadWindowPage checks the bounds direction', () {
      final newer = parseThreadWindowPage(
        [_reply(4), _reply(5), _bounds(hasMore: false, direction: 'newer')],
        channelId: _room,
        rootId: _root,
        direction: WindowPageDirection.newer,
      );
      expect(newer?.direction, WindowPageDirection.newer);
      expect(newer?.events.map((e) => e.createdAt), [4, 5]);
      expect(
        () => parseThreadWindowPage(
          [_reply(4), _bounds(hasMore: false)],
          channelId: _room,
          rootId: _root,
          direction: WindowPageDirection.newer,
        ),
        throwsFormatException,
      );
      expect(
        () => parseThreadWindowPage(
          [_reply(4), _bounds(hasMore: false, direction: 'newer')],
          channelId: _room,
          rootId: _root,
        ),
        throwsFormatException,
      );
    });
  });

  group('ThreadWindowNotifier opened at a reply', () {
    const args = ThreadRepliesArgs(channelId: _room, rootId: _root);

    test(
      'opens at a reply, survives a stale refresh, and pages to the newest',
      () async {
        final relay = _WindowRelay(200);
        final container = _container(relay);
        final shown = container.listen(
          threadRepliesWithLocalProvider(args),
          (_, _) {},
        );
        await container.read(threadRepliesProvider(args).future);
        final window = container.read(threadWindowProvider(args).notifier);
        List<int> held() => _ids(container.read(threadWindowProvider(args))!);

        expect(
          await window.openAt(createdAt: 180, eventId: _reply(180).id),
          isTrue,
        );
        expect(window.windowGeneration, 1);
        final preceding = windowAnchorCursor(
          createdAt: 180,
          eventId: _reply(180).id,
        ).eventId;
        final opening = relay.windowFilters.skip(1).toList();
        expect(opening[0].until, 180);
        expect(opening[0].extensions['before_id'], preceding);
        expect(opening[1].since, 180);
        expect(opening[1].until, isNull);
        expect(opening[1].extensions['after_id'], preceding);
        // The anchor leads the older page; the newer page follows it.
        expect(held(), _range(131, 200));
        expect(window.hasNewer, isFalse);

        // Reopen far enough back that newer pages remain.
        expect(
          await window.openAt(createdAt: 30, eventId: _reply(30).id),
          isTrue,
        );
        expect(window.windowGeneration, 2);
        expect(held(), _range(1, 80));
        expect(window.hasNewer, isTrue);

        // A live reply marks the thread stale; its newest-page refresh must
        // keep the window the reader is in.
        relay.total = 201;
        final fetched = relay.windowFilters.length;
        container.read(threadRepliesStaleProvider(args).notifier).markStale();
        await _settle(container);
        // The refresh did fetch the newest page (and found it disconnected).
        expect(relay.windowFilters.length, fetched + 1);
        expect(relay.windowFilters.last.until, isNull);
        expect(relay.windowFilters.last.since, isNull);
        expect(held(), _range(1, 80));
        expect(window.hasNewer, isTrue);
        expect(shown.read().value?.map((e) => e.createdAt), _range(1, 80));

        final firstNewer = relay.windowFilters.length;
        for (var i = 0; window.hasNewer; i++) {
          expect(i, lessThan(10));
          expect(await window.loadNewer(), isTrue);
        }
        await container.pump();
        expect(shown.read().value?.map((e) => e.createdAt), _range(1, 201));
        final newer = relay.windowFilters.skip(firstNewer).toList();
        expect(newer.length, 3);
        expect(newer.first.since, 80);
        expect(newer.first.extensions['after_id'], _reply(80).id);
        expect(newer.last.extensions['after_id'], _reply(180).id);
      },
    );

    test('without nip-cw-newer openAt leaves the window as it was', () async {
      final relay = _WindowRelay(120);
      final container = _container(relay, newer: false);
      container.listen(threadRepliesWithLocalProvider(args), (_, _) {});
      await container.read(threadRepliesProvider(args).future);
      final window = container.read(threadWindowProvider(args).notifier);
      final before = container.read(threadWindowProvider(args));

      expect(
        await window.openAt(createdAt: 30, eventId: _reply(30).id),
        isFalse,
      );
      expect(container.read(threadWindowProvider(args)), same(before));
      expect(relay.windowFilters.length, 1);
      expect(window.windowGeneration, 0);
      expect(await window.loadNewer(), isFalse);
      expect(await window.jumpToLatest(), isFalse);
    });

    test('jumpToLatest starts over at the newest page', () async {
      final relay = _WindowRelay(200);
      final container = _container(relay);
      container.listen(threadRepliesWithLocalProvider(args), (_, _) {});
      await container.read(threadRepliesProvider(args).future);
      final window = container.read(threadWindowProvider(args).notifier);
      await window.openAt(createdAt: 30, eventId: _reply(30).id);

      expect(await window.jumpToLatest(), isTrue);
      final latest = container.read(threadWindowProvider(args))!;
      expect(_ids(latest), _range(151, 200));
      expect(latest.state.hasNewer, isFalse);
      expect(latest.state.olderCursor, _cursor(151));
      expect(window.windowGeneration, 2);
    });
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
