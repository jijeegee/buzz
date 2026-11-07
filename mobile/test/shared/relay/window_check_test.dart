import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';

import 'package:kuzz/shared/relay/nostr_models.dart';
import 'package:kuzz/shared/relay/window_check.dart';

const _key = 'room-1';
const _kinds = [9];
const _limit = 3;

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

/// One fake window open: a store, a relay check and a page, counting what
/// was asked of each.
class _Fake {
  _Fake({
    required this.stored,
    required this.answers,
    this.page = const [],
    WindowCheckOverride override = WindowCheckOverride.follow,
    WindowFacts? afterReplace,
  }) : _afterReplace = afterReplace {
    gate = WindowCheckGate(now: () => now, override: () => this.override);
    this.override = override;
  }

  late final WindowCheckGate gate;
  late WindowCheckOverride override;
  DateTime now = DateTime(2026, 11, 8);

  /// `null`: not fully stored.
  WindowFacts? stored;
  final List<WindowCheckAnswer?> answers;
  List<NostrEvent> page;
  final WindowFacts? _afterReplace;
  int live = 0;
  void Function()? onCheck;

  int reads = 0;
  int checks = 0;
  int marks = 0;
  final replaced = <List<NostrEvent>?>[];
  final logs = <String>[];

  Future<WindowCheckDecision> decide() => decideWindowCheck(
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
          _row('stored', 1),
          _bounds(hasMore: hasMore),
        ],
      );
    },
    check: () async {
      checks++;
      onCheck?.call();
      return answers.removeAt(0);
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
    log: logs.add,
  );

  /// Opens the window as the providers do: from the store when the
  /// decision has it, else the page (counted).
  int fetches = 0;
  Future<List<NostrEvent>> open() async {
    final decision = gate.before(_key) == null
        ? WindowCheckDecision.today
        : await decide();
    if (decision.stored case final stored?) return stored;
    final events = await decision.relayed(
      Future(() {
        fetches++;
        return page;
      }),
    );
    // Let onPage finish.
    await pumpEventQueue();
    return events;
  }
}

final _page = [
  _row('a3', 3000),
  _row('a2', 2000),
  _row('a1', 1000),
  _bounds(hasMore: true),
];

void main() {
  test('equal in mode on: opened from the store, no page fetched', () async {
    final fake = _Fake(
      stored: (oldestId: 'a1', count: 3),
      answers: [_answer('a1', 3, WindowCheckMode.on)],
      page: _page,
    );
    // The session's first check (a probe beside the page) said on.
    fake.gate.noteAnswer(_answer(null, 0, WindowCheckMode.on));
    final events = await fake.open();
    expect(fake.fetches, 0);
    expect(fake.checks, 1);
    expect(events.first.id, 'stored');
    // A full window says the relay has more.
    expect(jsonDecode(events.last.content)['has_more'], isTrue);
    expect(fake.marks, 0);
  });

  test('a missing row refetches the page and replaces its range', () async {
    final fake = _Fake(
      stored: (oldestId: 'a0', count: 3),
      answers: [_answer('a1', 3, WindowCheckMode.on)],
      page: _page,
    );
    fake.gate.noteAnswer(_answer(null, 0, WindowCheckMode.on));
    final events = await fake.open();
    expect(fake.fetches, 1);
    expect(events, _page);
    expect(fake.replaced, [_page]);
    expect(fake.gate.isHeld(_key), isFalse);
  });

  test('a leftover deleted row refetches', () async {
    final fake = _Fake(
      // The relay deleted a2: it counts one row fewer, oldest further down.
      stored: (oldestId: 'a1', count: 3),
      answers: [_answer('a0', 3, WindowCheckMode.on)],
      page: _page,
    );
    fake.gate.noteAnswer(_answer(null, 0, WindowCheckMode.on));
    await fake.open();
    expect(fake.fetches, 1);
    expect(fake.replaced, hasLength(1));
  });

  test('a short window whose count differs refetches', () async {
    final fake = _Fake(
      stored: (oldestId: 'a1', count: 1),
      answers: [_answer('a1', 2, WindowCheckMode.on)],
      page: [_row('a2', 2000), _row('a1', 1000), _bounds(hasMore: false)],
    );
    fake.gate.noteAnswer(_answer(null, 0, WindowCheckMode.on));
    await fake.open();
    expect(fake.fetches, 1);
    expect(fake.replaced.single, hasLength(3));
  });

  test('a window not fully stored fetches its page as before', () async {
    final fake = _Fake(stored: null, answers: [], page: _page);
    fake.gate.noteAnswer(_answer(null, 0, WindowCheckMode.on));
    await fake.open();
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
    );
    fake.gate.noteAnswer(_answer(null, 0, WindowCheckMode.on));
    await fake.open();
    expect(fake.gate.isHeld(_key), isTrue);
    expect(fake.gate.before(_key), isNull);
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
    );
    fake.gate.noteAnswer(_answer(null, 0, WindowCheckMode.on));
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
    final events = await fake.open();
    expect(fake.fetches, 1);
    expect(events, _page);
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
    );
    fake.gate.noteAnswer(_answer(null, 0, WindowCheckMode.on));
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
    expect(first, _page);
    expect(fake.checks, 1);
    expect(fake.replaced, [null]);
    // Then off: no store read, no check, no mark.
    final reads = fake.reads;
    expect(fake.gate.before(_key), isNull);
    final second = await fake.open();
    expect(second, _page);
    expect(fake.reads, reads);
    expect(fake.checks, 1);
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
    );
    fake.gate.noteAnswer(_answer(null, 0, WindowCheckMode.on));
    final events = await fake.open();
    expect(events, _page);
    expect(fake.reads + fake.checks + fake.marks, 0);
  });

  test('a relay without the check is taken as off', () async {
    final fake = _Fake(
      stored: (oldestId: 'a1', count: 3),
      answers: [null],
      page: _page,
    );
    await fake.open();
    expect(fake.gate.before(_key), isNull);
    expect(fake.replaced, [null]);
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
    expect(
      windowCheckFilter(
        NostrFilter(kinds: const [9], limit: 3),
      ).toJson()['check'],
      isTrue,
    );
  });
}
