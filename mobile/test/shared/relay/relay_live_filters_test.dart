import 'dart:async';

import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/shared/relay/relay.dart';

const _status = NostrFilter(
  kinds: [30315],
  tags: {
    '#d': ['general'],
  },
  limit: 0,
);
NostrFilter _presence(List<String> authors) =>
    NostrFilter(kinds: const [20001], authors: authors, limit: 0);
final _a = 'a' * 64;
final _b = 'b' * 64;
final _c = 'c' * 64;

void main() {
  late _RecordingRelaySocket socket;
  late List<_ManualTimer> timers;
  late RelaySessionNotifier session;
  late List<RelaySubscriptionStatus> statuses;

  List<List<dynamic>> reqs() =>
      socket.messages.where((message) => message.first == 'REQ').toList();

  Future<LiveFiltersHandle> open(List<NostrFilter> filters) async {
    final pending = session.debugSubscribeFilters(
      filters,
      (_) {},
      onStatusChanged: statuses.add,
    );
    session.debugHandleMessage(['EOSE', 'l-1']);
    return pending;
  }

  setUp(() {
    socket = _RecordingRelaySocket();
    timers = [];
    statuses = [];
    session = RelaySessionNotifier(
      retryTimerFactory: (duration, callback) {
        final timer = _ManualTimer(duration, callback);
        timers.add(timer);
        return timer;
      },
    );
    session.debugAttachSocketForTest(socket);
  });

  test('bundle filters are live only, at most ten', () {
    expect(() => assertBundleLiveFilters([]), throwsArgumentError);
    expect(
      () => assertBundleLiveFilters([
        const NostrFilter(kinds: [1], limit: 5),
      ]),
      throwsArgumentError,
    );
    expect(
      () => assertBundleLiveFilters([
        const NostrFilter(
          kinds: [9],
          tags: {
            '#h': ['room'],
          },
          limit: 0,
        ),
      ]),
      throwsArgumentError,
    );
    expect(
      () => assertBundleLiveFilters([
        const NostrFilter(kinds: [1], since: 5, limit: 0),
      ]),
      throwsArgumentError,
    );
    expect(
      () => assertBundleLiveFilters([
        for (var i = 0; i < 11; i++) NostrFilter(kinds: [i], limit: 0),
      ]),
      throwsArgumentError,
    );
    assertBundleLiveFilters([
      _status,
      _presence([_a]),
    ]);
  });

  test('the opening REQ carries every filter', () async {
    await open([
      _status,
      _presence([_a]),
    ]);
    expect(reqs().single, [
      'REQ',
      'l-1',
      _status.toJson(),
      _presence([_a]).toJson(),
    ]);
    expect(statuses, [RelaySubscriptionStatus.ready]);
  });

  test('a setFilters EOSE is not a ready for listeners', () async {
    final handle = await open([
      _status,
      _presence([_a]),
    ]);
    var done = false;
    final update = handle
        .setFilters([
          _status,
          _presence([_a, _b]),
        ])
        .then((_) => done = true);
    expect(reqs(), hasLength(2));
    expect(reqs().last, [
      'REQ',
      'l-1',
      _status.toJson(),
      _presence([_a, _b]).toJson(),
    ]);
    await Future<void>.delayed(Duration.zero);
    expect(done, isFalse);
    session.debugHandleMessage(['EOSE', 'l-1']);
    await update;
    expect(done, isTrue);
    expect(statuses, [
      RelaySubscriptionStatus.ready,
    ], reason: 'presence would blank its snapshot on a second ready');
    // The same set again sends nothing.
    await handle.setFilters([
      _status,
      _presence([_a, _b]),
    ]);
    expect(reqs(), hasLength(2));
  });

  test('two setFilters before an EOSE send one more REQ, newest set', () async {
    final handle = await open([
      _status,
      _presence([_a]),
    ]);
    final first = handle.setFilters([
      _status,
      _presence([_a, _b]),
    ]);
    expect(reqs(), hasLength(2));
    final second = handle.setFilters([
      _status,
      _presence([_a, _b, _c]),
    ]);
    final third = handle.setFilters([
      _status,
      _presence([_a, _c]),
    ]);
    expect(reqs(), hasLength(2), reason: 'a REQ is in flight');
    session.debugHandleMessage(['EOSE', 'l-1']);
    await first;
    expect(reqs(), hasLength(3));
    expect(reqs().last[3], _presence([_a, _c]).toJson());
    session.debugHandleMessage(['EOSE', 'l-1']);
    await Future.wait([second, third]);
    expect(reqs(), hasLength(3));
  });

  test('the opening REQ counts as in flight', () async {
    final pending = session.debugSubscribeFilters(
      [
        _status,
        _presence([_a]),
      ],
      (_) {},
      onStatusChanged: statuses.add,
    );
    // The readiness fallback hands the handle out before the relay answered.
    final handle = await pending;
    final update = handle.setFilters([
      _status,
      _presence([_a, _b]),
    ]);
    expect(reqs(), hasLength(1));
    session.debugHandleMessage(['EOSE', 'l-1']);
    expect(reqs(), hasLength(2));
    expect(statuses, [RelaySubscriptionStatus.ready]);
    session.debugHandleMessage(['EOSE', 'l-1']);
    await update;
    expect(statuses, [RelaySubscriptionStatus.ready]);
  });

  test('an update while offline rides the replay, unchanged', () async {
    final handle = await open([
      _status,
      _presence([_a]),
    ]);
    session.debugMarkSocketDownForTest();
    final update = handle.setFilters([
      _status,
      _presence([_a, _b]),
    ]);
    expect(reqs(), hasLength(1), reason: 'nothing is sent while offline');
    session.debugAttachSocketForTest(socket);
    await session.debugReplayLiveSubscriptions();
    expect(reqs(), hasLength(2));
    expect(reqs().last, [
      'REQ',
      'l-1',
      _status.toJson(),
      _presence([_a, _b]).toJson(),
    ]);
    session.debugHandleMessage(['EOSE', 'l-1']);
    await update;
    expect(statuses, [
      RelaySubscriptionStatus.ready,
      RelaySubscriptionStatus.ready,
    ], reason: 'a replay EOSE is a ready');
  });

  test('a CLOSED retry re-sends the current set', () async {
    final handle = await open([
      _status,
      _presence([_a]),
    ]);
    session.debugHandleMessage(['CLOSED', 'l-1', 'error: relay overloaded']);
    expect(statuses.last, RelaySubscriptionStatus.retrying);
    final update = handle.setFilters([
      _status,
      _presence([_a, _b]),
    ]);
    expect(reqs(), hasLength(1), reason: 'the retry sends next');
    timers.single.fire();
    await Future<void>.delayed(Duration.zero);
    await Future<void>.delayed(Duration.zero);
    expect(reqs(), hasLength(2));
    expect(reqs().last[3], _presence([_a, _b]).toJson());
    session.debugHandleMessage(['EOSE', 'l-1']);
    await update;
    expect(statuses.last, RelaySubscriptionStatus.ready);
  });

  test('close completes pending updates and sends CLOSE', () async {
    final handle = await open([
      _status,
      _presence([_a]),
    ]);
    final first = handle.setFilters([
      _status,
      _presence([_a, _b]),
    ]);
    final second = handle.setFilters([
      _status,
      _presence([_a, _b, _c]),
    ]);
    handle.close();
    await Future.wait([first, second]);
    expect(
      socket.messages.where((m) => m.first == 'CLOSE' && m[1] == 'l-1'),
      hasLength(1),
    );
  });
}

class _RecordingRelaySocket extends RelaySocket {
  _RecordingRelaySocket()
    : super(
        wsUrl: 'wss://relay.example',
        nsec: null,
        onMessage: (_) {},
        onConnected: () {},
        onDisconnected: (_) {},
      );

  final List<List<dynamic>> messages = [];

  @override
  void send(List<dynamic> payload) => messages.add(payload);

  @override
  void dispose() {}
}

class _ManualTimer implements Timer {
  _ManualTimer(this.duration, this._callback);

  final Duration duration;
  final void Function() _callback;
  bool _active = true;

  void fire() {
    if (!_active) return;
    _active = false;
    _callback();
  }

  @override
  void cancel() => _active = false;

  @override
  bool get isActive => _active;

  @override
  int get tick => _active ? 0 : 1;
}
