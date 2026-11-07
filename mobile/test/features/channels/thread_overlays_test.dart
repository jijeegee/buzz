import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:kuzz/features/channels/channel_messages_provider.dart';
import 'package:kuzz/features/channels/thread_replies_provider.dart';
import 'package:kuzz/features/channels/thread_reply_ownership.dart';
import 'package:kuzz/features/channels/thread_window.dart';
import 'package:kuzz/features/channels/window_anchor.dart';
import 'package:kuzz/shared/relay/relay.dart';

// Reactions, edits and deletions on thread replies made while the thread was
// not open: the thread window carries them (NIP-CW `include_aux`) and keeps
// those that arrive live (docs/message-sync.md#placing-an-event).

const _room = '22222222-2222-4222-8222-222222222222';
const _args = ThreadRepliesArgs(channelId: _room, rootId: 'root');

NostrEvent _event(String id, int createdAt, {String? parent}) => NostrEvent(
  id: id,
  pubkey: 'alice',
  createdAt: createdAt,
  kind: EventKind.streamMessageV2,
  tags: [
    ['h', _room],
    if (parent != null) ...[
      ['e', 'root', '', 'root'],
      ['e', parent, '', 'reply'],
    ],
  ],
  content: id,
  sig: 'sig',
);

NostrEvent _reply(String id, int createdAt) =>
    _event(id, createdAt, parent: 'root');

/// A reaction (no `h` tag, as clients send them) on [target].
NostrEvent _reaction(String id, String target, int createdAt) => NostrEvent(
  id: id,
  pubkey: 'carol',
  createdAt: createdAt,
  kind: EventKind.reaction,
  tags: [
    ['e', target],
  ],
  content: '👀',
  sig: 'sig',
);

NostrEvent _deletion(String id, String target, int createdAt) => NostrEvent(
  id: id,
  pubkey: 'carol',
  createdAt: createdAt,
  kind: EventKind.deletion,
  tags: [
    ['e', target],
  ],
  content: '',
  sig: 'sig',
);

NostrEvent _bounds({bool hasMore = false, NostrEvent? next}) => NostrEvent(
  id: 'bounds',
  pubkey: 'relay',
  createdAt: 0,
  kind: threadWindowBoundsKind,
  tags: const [
    ['h', _room],
    ['e', 'root'],
  ],
  content: jsonEncode({
    'version': 1,
    'direction': 'older',
    'has_more': hasMore,
    'next_cursor': next == null
        ? null
        : {'created_at': next.createdAt, 'id': next.id},
  }),
  sig: '',
);

List<String> _ids(Iterable<NostrEvent>? events) => [
  for (final event in events ?? const <NostrEvent>[]) event.id,
];

RelayException _overBudget() => RelayException(
  503,
  jsonEncode({'error': 'thread window over budget', 'code': 'window_budget'}),
);

/// Serves the room window and the thread window with its aux closure.
class _Relay extends RelaySessionNotifier {
  final requests = <String>[];
  final _listeners = <void Function(NostrEvent)>[];
  List<NostrEvent> threadRows = [_reply('r1', 20)];
  List<NostrEvent> threadAux = [];

  @override
  SessionState build() => const SessionState(status: SessionStatus.connected);

  @override
  Future<List<NostrEvent>> queryRelay(
    List<NostrFilter> filters, {
    Duration timeout = const Duration(seconds: 8),
  }) async {
    final extensions = filters.single.extensions;
    if (extensions['top_level'] == true) {
      requests.add('room window');
      return [
        _event('root', 10),
        NostrEvent(
          id: 'room-bounds',
          pubkey: 'relay',
          createdAt: 0,
          kind: EventKind.channelWindowBounds,
          tags: [
            ['d', '$_room:head'],
          ],
          content: jsonEncode({'has_more': false, 'next_cursor': null}),
          sig: 'sig',
        ),
      ];
    }
    if (extensions['thread_window'] == true) {
      requests.add('thread window aux=${extensions['include_aux']}');
      return [
        ...threadRows,
        if (extensions['include_aux'] == true) ...threadAux,
        _bounds(),
      ];
    }
    requests.add('other');
    return const [];
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
    void Function()? onGap,
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

/// Serves a 202-reply thread (0..201) to the complete scan, a reaction on its first
/// page when asked for the aux closure.
class _ScanRelay extends RelaySessionNotifier {
  final asked = <NostrFilter>[];

  @override
  SessionState build() => const SessionState(status: SessionStatus.connected);

  @override
  Future<List<NostrEvent>> queryRelay(
    List<NostrFilter> filters, {
    Duration timeout = const Duration(seconds: 8),
  }) async {
    final filter = filters.single;
    asked.add(filter);
    final from = (filter.extensions['thread_cursor'] as int) + 1;
    final rows = [
      for (var n = from; n <= 201 && n < from + 200; n++)
        _reply('r$n'.padLeft(4, '0'), n),
    ];
    return [
      ...rows,
      if (filter.extensions['include_aux'] == true && from == 0)
        _reaction('x', rows.first.id, 300),
    ];
  }
}

Future<void> _settle() async {
  for (var i = 0; i < 6; i++) {
    await Future<void>.delayed(Duration.zero);
  }
}

/// The room open and the thread opened once, then closed: the window stays
/// held (closed-thread retention) while nothing shows it.
Future<(ProviderContainer, _Relay, void Function())> _openThread() async {
  final relay = _Relay();
  final container = ProviderContainer(
    overrides: [
      relaySessionProvider.overrideWith(() => relay),
      relayInfoProvider.overrideWith(
        (ref) async =>
            const RelayInfo(supportedExtensions: {newerWindowExtension}),
      ),
    ],
  );
  addTearDown(container.dispose);
  container.listen(channelMessagesProvider(_room), (_, _) {});
  await _settle();
  final shown = container.listen(
    threadRepliesWithLocalProvider(_args),
    (_, _) {},
  );
  await container.read(threadRepliesProvider(_args).future);
  await _settle();
  return (container, relay, shown.close);
}

List<String> _held(ProviderContainer container) =>
    _ids(container.read(threadWindowProvider(_args))?.events);

void main() {
  group('thread window pages', () {
    test('the parser splits rows from their aux closure', () {
      final page = parseThreadWindowPage(
        [
          _reply('r2', 30),
          _reply('r1', 20),
          _reaction('x', 'r1', 40),
          _deletion('dx', 'x', 41),
          _bounds(),
        ],
        channelId: _room,
        rootId: 'root',
      )!;
      expect(_ids(page.events), ['r2', 'r1']);
      expect(_ids(page.overlays), ['x', 'dx']);
    });

    test('every merge keeps the page overlays beside its rows', () {
      final newest = ThreadWindowPage(
        events: [_reply('r1', 20)],
        overlays: [_reaction('x', 'r1', 40)],
        hasMore: true,
        nextCursor: ThreadWindowCursor.of(_reply('r1', 20)),
      );
      final first = mergeNewestThreadPage(null, newest);
      expect(_ids(first.events), containsAll(['r1', 'x']));

      final older = mergeOlderThreadPage(
        first,
        ThreadWindowPage(
          events: [_reply('r0', 15)],
          overlays: [_deletion('d0', 'r0', 42)],
          hasMore: false,
        ),
      );
      expect(_ids(older.events), containsAll(['r0', 'r1', 'x', 'd0']));

      final refreshed = mergeNewestThreadPage(
        older,
        ThreadWindowPage(
          events: [_reply('r1', 20)],
          overlays: [_reaction('x', 'r1', 40), _reaction('y', 'r1', 43)],
          hasMore: true,
          nextCursor: ThreadWindowCursor.of(_reply('r1', 20)),
        ),
      );
      // The refreshed page brings a new reaction; the older page keeps its.
      expect(_ids(refreshed.events), containsAll(['r0', 'r1', 'x', 'y', 'd0']));
      expect(_ids(refreshed.events).where((id) => id == 'x'), hasLength(1));
    });

    test('a live aux event folds only into a held window', () {
      expect(foldLiveThreadAux(null, _reaction('x', 'r1', 40)), isNull);
      final window = mergeNewestThreadPage(
        null,
        ThreadWindowPage(events: [_reply('r1', 20)], hasMore: false),
      );
      expect(foldLiveThreadAux(window, _reply('r2', 30)), isNull);
      final folded = foldLiveThreadAux(window, _reaction('x', 'r1', 40))!;
      expect(_ids(folded.events), ['r1', 'x']);
      expect(folded.state, same(window.state));
      // The same event twice changes nothing.
      expect(foldLiveThreadAux(folded, _reaction('x', 'r1', 40)), same(folded));
    });

    test('the window filter asks for the aux closure', () {
      final filter = threadWindowFilter(channelId: _room, rootId: 'root');
      expect(filter.extensions['include_aux'], true);
    });
  });

  group('a window over the relay budget', () {
    test('is asked again smaller, keeping its aux closure', () async {
      final asked = <NostrFilter>[];
      final events = await queryThreadWindow(
        (filters) async {
          asked.add(filters.single);
          if (asked.length == 1) throw _overBudget();
          return [_reply('r1', 20), _reaction('x', 'r1', 40), _bounds()];
        },
        threadWindowFilter(channelId: _room, rootId: 'root'),
        rootId: 'root',
      );
      expect(
        [for (final f in asked) f.limit],
        [threadWindowPageReplies, threadWindowReducedReplies],
      );
      expect(asked.last.extensions['include_aux'], true);
      expect(_ids(events), ['r1', 'x', 'bounds']);
    });

    test('then without it, asking its overlays by id', () async {
      final asked = <NostrFilter>[];
      final events = await queryThreadWindow(
        (filters) async {
          final filter = filters.single;
          asked.add(filter);
          if (filter.extensions['thread_window'] == true) {
            if (filter.extensions['include_aux'] == true) throw _overBudget();
            return [_reply('r1', 20), _bounds()];
          }
          final targets = filter.tags['#e']!;
          if (targets.contains('r1')) return [_reaction('x', 'r1', 40)];
          if (targets.contains('x')) return [_deletion('dx', 'x', 41)];
          return const [];
        },
        threadWindowFilter(channelId: _room, rootId: 'root'),
        rootId: 'root',
      );
      expect(asked[2].extensions['include_aux'], false);
      expect(asked[3].tags['#e'], ['r1', 'root']);
      expect(asked[4].tags['#e'], ['x']);
      // Overlays sit before the bounds, as in a relay's aux closure.
      expect(_ids(events), ['r1', 'x', 'dx', 'bounds']);
    });

    test('asks removals of the overlays it holds and pages', () async {
      final asked = <NostrFilter>[];
      final events = await queryThreadWindow(
        (filters) async {
          final filter = filters.single;
          asked.add(filter);
          if (filter.extensions['thread_window'] == true) {
            if (filter.extensions['include_aux'] == true) throw _overBudget();
            return [_reply('r1', 20), _bounds()];
          }
          final targets = filter.tags['#e']!;
          if (targets.contains('r1')) {
            // A full page, then one more reaction older than it.
            final until = filter.until;
            if (until == null) {
              return [
                for (var n = 0; n < 500; n++) _reaction('a$n', 'r1', 1000 - n),
              ];
            }
            return [_reaction('a499', 'r1', 501), _reaction('old', 'r1', 400)];
          }
          // The held reaction was withdrawn; the relay no longer serves it.
          if (targets.contains('held')) return [_deletion('dh', 'held', 50)];
          return const [];
        },
        threadWindowFilter(channelId: _room, rootId: 'root'),
        rootId: 'root',
        held: const ['held'],
      );
      expect(asked[4].until, 501);
      expect(asked.last.tags['#e'], contains('held'));
      final ids = _ids(events);
      expect(ids, containsAll(['a0', 'a499', 'old', 'dh']));
      expect(ids.where((id) => id == 'a499'), hasLength(1));
      expect(ids.last, 'bounds');
    });

    test('keeps its rows when the overlay request fails', () async {
      final events = await queryThreadWindow(
        (filters) async {
          final filter = filters.single;
          if (filter.extensions['thread_window'] != true) {
            throw Exception('relay unavailable');
          }
          if (filter.extensions['include_aux'] == true) throw _overBudget();
          return [_reply('r1', 20), _bounds()];
        },
        threadWindowFilter(channelId: _room, rootId: 'root'),
        rootId: 'root',
      );
      expect(_ids(events), ['r1', 'bounds']);
    });

    test('other errors are not retried', () async {
      var calls = 0;
      await expectLater(
        queryThreadWindow(
          (filters) async {
            calls++;
            throw RelayException(503, '{"error":"busy"}');
          },
          threadWindowFilter(channelId: _room, rootId: 'root'),
          rootId: 'root',
        ),
        throwsA(isA<RelayException>()),
      );
      expect(calls, 1);
      expect(isWindowBudgetError(_overBudget()), isTrue);
      expect(isWindowBudgetError(RelayException(500, '{}')), isFalse);
    });
  });

  test('a complete scan collects overlays apart from its pages', () async {
    final relay = _ScanRelay();
    final container = ProviderContainer(
      overrides: [relaySessionProvider.overrideWith(() => relay)],
    );
    addTearDown(container.dispose);
    final session = container.read(relaySessionProvider.notifier);

    final overlays = <NostrEvent>[];
    final replies = await fetchCompleteThreadReplies(
      session,
      _args,
      overlays: overlays,
    );
    // A full page of replies plus aux still pages on: aux is not counted.
    expect(replies, hasLength(202));
    expect(_ids(overlays), ['x']);
    expect(
      [for (final f in relay.asked) f.extensions['include_aux']],
      [true, true],
    );

    relay.asked.clear();
    final recount = await fetchCompleteThreadReplies(session, _args);
    expect(recount, hasLength(202));
    expect(relay.asked.first.extensions.containsKey('include_aux'), isFalse);
  });

  test('ownership remembers which thread holds an overlay', () {
    final ownership = ThreadReplyOwnership()
      ..recordOverlays('root', [_reaction('x', 'r1', 40)]);
    expect(ownership.overlayRootFor('x'), 'root');
    expect(ownership.overlayRootFor('r1'), isNull);
  });

  group('a held thread', () {
    test('opens with reactions made while it was closed', () async {
      final relay = _Relay()..threadAux = [_reaction('x', 'r1', 40)];
      final container = ProviderContainer(
        overrides: [relaySessionProvider.overrideWith(() => relay)],
      );
      addTearDown(container.dispose);
      container.listen(channelMessagesProvider(_room), (_, _) {});
      await _settle();
      container.listen(threadRepliesWithLocalProvider(_args), (_, _) {});
      await container.read(threadRepliesProvider(_args).future);
      expect(relay.requests, contains('thread window aux=true'));
      expect(_held(container), containsAll(['r1', 'x']));
    });

    test('keeps a reaction made while closed and shows it reopened', () async {
      final (container, relay, close) = await _openThread();
      close();
      await _settle();
      relay.requests.clear();

      relay.emit(_reaction('x', 'r1', 40));
      await _settle();
      expect(_held(container), containsAll(['r1', 'x']));

      // Reopened within retention: no request, the reaction is there.
      container.listen(threadRepliesWithLocalProvider(_args), (_, _) {});
      await container.read(threadRepliesProvider(_args).future);
      await _settle();
      expect(relay.requests, isEmpty);
      final shown = container.read(threadRepliesWithLocalProvider(_args)).value;
      expect(_ids(shown), containsAll(['r1', 'x']));
    });

    test('a removed reaction reaches the thread by the reaction id', () async {
      final (container, relay, close) = await _openThread();
      relay.emit(_reaction('x', 'r1', 40));
      await _settle();
      close();
      await _settle();

      // The deletion names only the reaction, never the reply.
      relay.emit(_deletion('dx', 'x', 41));
      await _settle();
      expect(_held(container), containsAll(['r1', 'x', 'dx']));
    });

    test('a room refresh keeps the thread overlays', () async {
      final (container, relay, _) = await _openThread();
      relay.emit(_reaction('x', 'r1', 40));
      await _settle();

      container.invalidate(channelMessagesProvider(_room));
      await _settle();
      expect(relay.requests.where((r) => r == 'room window'), hasLength(2));
      expect(_held(container), containsAll(['r1', 'x']));
    });

    test('a reaction that arrives before the first page is kept', () async {
      final relay = _Relay();
      final container = ProviderContainer(
        overrides: [relaySessionProvider.overrideWith(() => relay)],
      );
      addTearDown(container.dispose);
      container.listen(channelMessagesProvider(_room), (_, _) {});
      await _settle();
      container.listen(threadWindowProvider(_args), (_, _) {});
      final window = container.read(threadWindowProvider(_args).notifier);
      expect(window.foldLiveAux(_reaction('x', 'root', 40)), isTrue);
      expect(container.read(threadWindowProvider(_args)), isNull);

      container.listen(threadRepliesWithLocalProvider(_args), (_, _) {});
      await container.read(threadRepliesProvider(_args).future);
      expect(_held(container), containsAll(['r1', 'x']));
    });

    test('a complete scan keeps what arrived live while it ran', () {
      final container = ProviderContainer();
      addTearDown(container.dispose);
      container.listen(threadWindowProvider(_args), (_, _) {});
      final window = container.read(threadWindowProvider(_args).notifier)
        ..replaceComplete([_reply('r1', 20)], [_reaction('old', 'r1', 30)])
        ..beginScan();
      window.foldLiveAux(_reaction('x', 'r1', 40));
      window.replaceComplete([_reply('r1', 20)], [_reaction('y', 'r1', 35)]);
      // The scan's overlays replace the held ones; the live one stays.
      expect(_held(container), ['r1', 'y', 'x']);

      // A scan without overlays keeps those held.
      window
        ..beginScan()
        ..replaceComplete([_reply('r1', 20), _reply('r2', 50)], null);
      expect(_held(container), ['r1', 'y', 'x', 'r2']);
    });

    test('a reaction on another thread stays out', () async {
      final (container, relay, _) = await _openThread();
      relay.emit(_reaction('y', 'elsewhere', 40));
      await _settle();
      expect(_held(container), isNot(contains('y')));
    });
  });
}
