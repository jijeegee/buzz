import 'dart:async';
import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';

import 'package:kuzz/shared/relay/nostr_models.dart';
import 'package:kuzz/shared/relay/window_check.dart';

const _key = 'room-1';
const _kinds = [9];
const _limit = 3;
final _filter = NostrFilter(kinds: _kinds, limit: _limit);

NostrEvent _row(String id, int at) => NostrEvent(
  id: id,
  pubkey: 'p1',
  createdAt: at,
  kind: 9,
  tags: const [
    ['h', 'room-1'],
  ],
  content: '',
  sig: '',
);

NostrEvent _bounds({required bool hasMore, Map<String, Object?>? check}) =>
    NostrEvent(
      id: '0' * 64,
      pubkey: '0' * 64,
      createdAt: 0,
      kind: EventKind.channelWindowBounds,
      tags: const [
        ['d', 'room-1:head'],
      ],
      content: jsonEncode({'has_more': hasMore, 'check': ?check}),
      sig: '',
    );

WindowCheckAnswer _answer(String? oldest, int count, WindowCheckMode mode) =>
    WindowCheckAnswer(oldestId: oldest, count: count, mode: mode);

/// The relay's answer to a check: the bounds event alone.
List<NostrEvent> _checked(WindowCheckAnswer answer) => [
  _bounds(
    hasMore: false,
    check: {
      'oldest_id': answer.oldestId,
      'count': answer.count,
      'mode': answer.mode.name,
    },
  ),
];

/// One fake window open: a store, a relay check and a page, counting what
/// was asked of each.
class _Fake {
  _Fake({
    required this.stored,
    required this.answers,
    this.page = const [],
    this.override = WindowCheckOverride.follow,
    WindowFacts? afterReplace,
  }) : _afterReplace = afterReplace {
    gate = WindowCheckGate(now: () => now, override: () => override);
  }

  late final WindowCheckGate gate;
  WindowCheckOverride override;
  DateTime now = DateTime(2026, 11, 8);

  /// `null`: not fully stored.
  WindowFacts? stored;

  /// Each check's answer: a check answer, raw events (a relay that does not
  /// know the check), or an error to throw.
  final List<Object> answers;
  List<NostrEvent> page;
  final WindowFacts? _afterReplace;
  int live = 0;
  void Function()? onCheck;

  /// Holds the check's answer back until completed.
  Completer<void>? checkHeld;

  int reads = 0;
  int checks = 0;
  int marks = 0;
  int fetches = 0;
  final replaced = <List<NostrEvent>?>[];
  final logs = <String>[];

  /// The store's window, as the open drew it.
  final storedWindow = [_row('stored', 1)];

  CheckedWindowOpen? begin() => openWindowCheck(
    gate: gate,
    key: _key,
    label: 'room $_key',
    kinds: _kinds,
    limit: _limit,
    readStored: () async {
      reads++;
      final facts = stored;
      if (facts == null) return null;
      return CheckedStoreWindow(
        facts: facts,
        response: ({required hasMore}) => [
          ...storedWindow,
          _bounds(hasMore: hasMore),
        ],
      );
    },
    check: (sent) async {
      checks++;
      onCheck?.call();
      await checkHeld?.future;
      final answer = answers.removeAt(0);
      return switch (answer) {
        final WindowCheckAnswer answer => _checked(answer),
        final List<NostrEvent> events => events,
        _ => throw answer,
      };
    },
    beginReplace: () {
      marks++;
      return (events) async {
        replaced.add(events);
        if (events == null) return null;
        return _afterReplace ?? pageWindowFacts(events, _kinds).facts;
      };
    },
    liveRows: () => live,
    fetch: (filters) async {
      fetches++;
      return page;
    },
    log: logs.add,
  );

  /// Opens the window as the providers do: draws the store's window when
  /// the open has one, then takes the page through the open's query.
  Future<({List<NostrEvent>? drawn, List<NostrEvent> page})> open() async {
    final open = begin();
    if (open == null) {
      fetches++;
      return (drawn: null, page: page);
    }
    final drawn = await open.stored;
    final events = await open.query([_filter]);
    // Let the work after the page finish.
    await pumpEventQueue();
    return (drawn: drawn, page: events);
  }

  /// The relay said on in an earlier check this session.
  void serverOn() => gate.noteAnswer(_answer(null, 0, WindowCheckMode.on));
}

final _page = [
  _row('a3', 3000),
  _row('a2', 2000),
  _row('a1', 1000),
  _bounds(hasMore: true),
];

bool _hasMore(List<NostrEvent> events) =>
    jsonDecode(events.last.content)['has_more'] == true;

void main() {
  test('mode on draws the store at once, before the check is sent', () async {
    final fake = _Fake(
      stored: (oldestId: 'a1', count: 3),
      answers: [_answer('a1', 3, WindowCheckMode.on)],
      page: _page,
    )..serverOn();
    fake.checkHeld = Completer();
    final open = fake.begin()!;
    final drawn = await open.stored;
    expect(drawn!.first.id, 'stored');
    expect(fake.checks, 0);
    // The check goes out with the page's query and holds only it.
    final page = open.query([_filter]);
    await pumpEventQueue();
    expect(fake.checks, 1);
    fake.checkHeld!.complete();
    final events = await page;
    // Equal: the stored window answers again; no page was fetched.
    expect(events.first.id, 'stored');
    expect(_hasMore(events), isTrue);
    expect(fake.fetches, 0);
    expect(fake.marks, 0);
  });

  test('a missing row: drawn from the store, then refetched', () async {
    final fake = _Fake(
      stored: (oldestId: 'a0', count: 3),
      answers: [_answer('a1', 3, WindowCheckMode.on)],
      page: _page,
    )..serverOn();
    final opened = await fake.open();
    expect(opened.drawn!.first.id, 'stored');
    expect(opened.page, _page);
    expect(fake.fetches, 1);
    expect(fake.replaced, [_page]);
    expect(fake.gate.isHeld(_key), isFalse);
  });

  test('a leftover deleted row refetches', () async {
    final fake = _Fake(
      // The relay deleted a row: its oldest is further down.
      stored: (oldestId: 'a1', count: 3),
      answers: [_answer('a0', 3, WindowCheckMode.on)],
      page: _page,
    )..serverOn();
    await fake.open();
    expect(fake.fetches, 1);
    expect(fake.replaced, hasLength(1));
  });

  test('a short window whose count differs refetches', () async {
    final fake = _Fake(
      stored: (oldestId: 'a1', count: 1),
      answers: [_answer('a1', 2, WindowCheckMode.on)],
      page: [_row('a2', 2000), _row('a1', 1000), _bounds(hasMore: false)],
    )..serverOn();
    await fake.open();
    expect(fake.fetches, 1);
    expect(fake.replaced.single, hasLength(3));
  });

  test('a window not fully stored draws as before and fetches', () async {
    final fake = _Fake(stored: null, answers: [], page: _page)..serverOn();
    final opened = await fake.open();
    expect(opened.drawn, isNull);
    expect(fake.checks, 0);
    expect(fake.fetches, 1);
    expect(fake.replaced, [_page]);
  });

  test('still different after the refetch: old path for 10 minutes', () async {
    final fake = _Fake(
      stored: (oldestId: 'a0', count: 3),
      answers: [_answer('a1', 3, WindowCheckMode.on)],
      page: _page,
      afterReplace: (oldestId: 'a0', count: 3),
    )..serverOn();
    await fake.open();
    expect(fake.gate.isHeld(_key), isTrue);
    expect(fake.begin(), isNull);
    // Another window is not held.
    expect(fake.gate.before('room-2'), WindowCheckMode.on);
    fake.now = fake.now.add(windowCheckRetry);
    expect(fake.gate.before(_key), WindowCheckMode.on);
  });

  test('rows arriving during a different check: checked once more', () async {
    final fake = _Fake(
      stored: (oldestId: 'a0', count: 3),
      answers: [
        _answer('a1', 3, WindowCheckMode.on),
        _answer('a1', 3, WindowCheckMode.on),
      ],
      page: _page,
    )..serverOn();
    fake.onCheck = () {
      fake.live++;
      fake.stored = (oldestId: 'a1', count: 3);
    };
    await fake.open();
    expect(fake.checks, 2);
    expect(fake.fetches, 0);
  });

  test('shadow fetches the page anyway and logs the result', () async {
    final fake = _Fake(
      stored: (oldestId: 'a1', count: 3),
      answers: [_answer('a1', 3, WindowCheckMode.shadow)],
      page: _page,
    );
    final opened = await fake.open();
    // Shadow draws as before: the open offers no store window.
    expect(opened.drawn, isNull);
    expect(opened.page, _page);
    expect(fake.fetches, 1);
    expect(fake.checks, 1);
    expect(fake.logs.single, contains('shadow equal'));
    // Equal: the mark is only released.
    expect(fake.replaced, [null]);
  });

  test('shadow different: the page replaces the stored range', () async {
    final fake = _Fake(
      stored: (oldestId: 'a0', count: 3),
      answers: [_answer('a1', 3, WindowCheckMode.shadow)],
      page: _page,
    );
    await fake.open();
    expect(fake.fetches, 1);
    expect(fake.replaced, [_page]);
    expect(fake.logs.single, contains('shadow different'));
  });

  test('the override in shadow fetches even when the relay says on', () async {
    final fake = _Fake(
      stored: (oldestId: 'a1', count: 3),
      answers: [_answer('a1', 3, WindowCheckMode.on)],
      page: _page,
      override: WindowCheckOverride.shadow,
    )..serverOn();
    await fake.open();
    expect(fake.fetches, 1);
  });

  test('mode off: opened as before, asked again after 10 minutes', () async {
    final fake = _Fake(
      stored: (oldestId: 'a1', count: 3),
      answers: [_answer(null, 0, WindowCheckMode.off)],
      page: _page,
    );
    // The first open of a session probes beside the page.
    final first = await fake.open();
    expect(first.page, _page);
    expect(fake.checks, 1);
    expect(fake.replaced, [null]);
    // Then off: no open, so no store read, check or mark.
    expect(fake.begin(), isNull);
    expect(fake.reads, 1);
    expect(fake.marks, 1);
    fake.now = fake.now.add(windowCheckRetry);
    expect(fake.gate.before(_key), WindowCheckMode.shadow);
  });

  test('the override off never checks', () async {
    final fake = _Fake(
      stored: (oldestId: 'a1', count: 3),
      answers: [],
      page: _page,
      override: WindowCheckOverride.off,
    )..serverOn();
    expect(fake.begin(), isNull);
    expect(fake.reads + fake.checks + fake.marks, 0);
  });

  test('a room relay without the check: its page answer is used', () async {
    final fake = _Fake(
      stored: (oldestId: 'a1', count: 3),
      // The relay ignores `check` and answers the page.
      answers: [_page],
      page: _page,
      override: WindowCheckOverride.on,
    );
    final opened = await fake.open();
    expect(opened.page, _page);
    expect(fake.fetches, 0);
    // Not asked again for 10 minutes, whatever the override.
    expect(fake.begin(), isNull);
    fake.now = fake.now.add(windowCheckRetry);
    expect(fake.gate.before(_key), WindowCheckMode.on);
  });

  test('a thread relay failing the check: the page as before, '
      'no error', () async {
    final fake = _Fake(
      stored: (oldestId: 'a1', count: 3),
      answers: [StateError('unknown field check')],
      page: _page,
      override: WindowCheckOverride.on,
    );
    final opened = await fake.open();
    expect(opened.page, _page);
    expect(fake.fetches, 1);
    expect(fake.begin(), isNull);
  });

  test('a shadow probe failing the check changes nothing', () async {
    final fake = _Fake(
      stored: (oldestId: 'a1', count: 3),
      answers: [StateError('unknown field check')],
      page: _page,
    );
    final opened = await fake.open();
    expect(opened.page, _page);
    expect(fake.fetches, 1);
    expect(fake.replaced, [null]);
    expect(fake.begin(), isNull);
  });

  test('later calls of the query are plain fetches', () async {
    final fake = _Fake(
      stored: (oldestId: 'a1', count: 3),
      answers: [_answer('a1', 3, WindowCheckMode.on)],
      page: _page,
    )..serverOn();
    final open = fake.begin()!;
    await open.query([_filter]);
    await open.query([_filter]);
    expect(fake.checks, 1);
    expect(fake.fetches, 1);
  });

  test('the check answer is read from the bounds event', () {
    final answer = parseWindowCheckAnswer([
      _row('a1', 1000),
      _bounds(
        hasMore: false,
        check: {
          'oldest_id': 'a1',
          'oldest_created_at': 1000,
          'count': 1,
          'room_seq': 7,
          'mode': 'shadow',
        },
      ),
    ])!;
    expect(answer.oldestId, 'a1');
    expect(answer.count, 1);
    expect(answer.roomSeq, 7);
    expect(answer.mode, WindowCheckMode.shadow);
    expect(parseWindowCheckAnswer([_bounds(hasMore: false)]), isNull);
    expect(windowCheckFilter(_filter).toJson()['check'], isTrue);
  });
}
