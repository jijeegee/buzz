import 'dart:async';

import 'package:fake_async/fake_async.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/shared/relay/me_live_bundle.dart';
import 'package:kuzz/shared/relay/relay.dart';

final _me = 'a' * 64;

void main() {
  test('one scope shares one REQ; listeners get their documents', () async {
    final relay = _FakeBundleRelay();
    final bundle = relay.bundle();
    final mutes = <String>[];
    final stars = <String>[];
    final muting = bundle.subscribe(
      _doc('channel-mutes'),
      (event) => mutes.add(event.id),
    );
    final starring = bundle.subscribe(
      _doc('channel-stars'),
      (event) => stars.add(event.id),
    );
    await pumpEventQueue();
    expect(relay.opened, hasLength(1));
    expect(relay.opened.single.filter.kinds, meAuthoredKinds);
    expect(relay.opened.single.filter.authors, [_me]);
    expect(relay.opened.single.filter.limit, 0);
    relay.opened.single.release();
    await Future.wait([muting, starring]);

    final req = relay.opened.single;
    req.onEvent(
      _event('m1', 30078, [
        ['d', 'channel-mutes'],
      ]),
    );
    req.onEvent(
      _event('s1', 30078, [
        ['d', 'channel-stars'],
      ]),
    );
    req.onEvent(
      _event('m1', 30078, [
        ['d', 'channel-mutes'],
      ]),
    );
    expect(mutes, ['m1']);
    expect(stars, ['s1']);
    expect(relay.fetches, isEmpty, reason: 'limit 0 asks for no history');
  });

  test('a filter asking for history is not the bundle\'s', () async {
    final bundle = _FakeBundleRelay().bundle();
    final history = NostrFilter(
      kinds: const [44100],
      tags: {
        '#p': [_me],
      },
      limit: 50,
    );
    await expectLater(bundle.subscribe(history, (_) {}), throwsArgumentError);
  });

  test('last listener leaving closes the REQ unless one comes back', () async {
    final relay = _FakeBundleRelay();
    final bundle = relay.bundle();
    final subscribing = bundle.subscribe(_doc('channel-sort'), (_) {});
    await pumpEventQueue();
    relay.opened.single.release();
    final stop = await subscribing;

    // A provider rebuild: dispose, then subscribe again in the same task.
    stop();
    final again = bundle.subscribe(_doc('channel-sort'), (_) {});
    await again;
    await pumpEventQueue();
    expect(relay.opened, hasLength(1));
    expect(relay.opened.single.closed, isFalse);

    (await again)();
    await pumpEventQueue();
    expect(relay.opened.single.closed, isTrue);
  });

  test('a failed REQ fails its listeners and the next one retries', () async {
    final relay = _FakeBundleRelay();
    final bundle = relay.bundle();
    final first = bundle.subscribe(_doc('people-favorites'), (_) {});
    await pumpEventQueue();
    relay.opened.single.fail();
    await expectLater(first, throwsException);

    final second = bundle.subscribe(_doc('people-favorites'), (_) {});
    await pumpEventQueue();
    expect(relay.opened, hasLength(2));
    relay.opened.last.release();
    await second;
  });

  test('a terminal CLOSED reaches listeners and frees the scope', () async {
    final relay = _FakeBundleRelay();
    final bundle = relay.bundle();
    final closed = <String>[];
    final subscribing = bundle.subscribe(
      _doc('thread-pins'),
      (_) {},
      onClosed: closed.add,
    );
    await pumpEventQueue();
    relay.opened.single.release();
    await subscribing;

    relay.opened.single.onClosed('restricted');
    expect(closed, ['restricted']);
    bundle.subscribe(_doc('thread-pins'), (_) {});
    await pumpEventQueue();
    expect(relay.opened, hasLength(2));
  });

  test('reconnect catch-up delivers what was missed, owing failures', () {
    fakeAsync((async) {
      final relay = _FakeBundleRelay();
      final bundle = relay.bundle();
      final names = <String>[];
      bundle.subscribe(_doc('channel-names'), (e) => names.add(e.id));
      async.flushMicrotasks();
      relay.opened.single.release();
      async.flushMicrotasks();
      relay.opened.single.onEvent(
        _event('n1', 30078, [
          ['d', 'channel-names'],
        ]),
      );

      relay.fetchFailures = 3;
      bundle.catchUp(1000);
      async.elapse(const Duration(seconds: 10));
      expect(relay.fetches, hasLength(3));

      relay.fetchResult = [
        _event('n1', 30078, [
          ['d', 'channel-names'],
        ]),
        _event('n2', 30078, [
          ['d', 'channel-names'],
        ], createdAt: 1500),
        _event('x', 30078, [
          ['d', 'channel-sort'],
        ]),
      ];
      bundle.catchUp(2000);
      async.flushMicrotasks();
      expect(relay.fetches.last.since, 1000, reason: 'the owed window');
      expect(relay.fetches.last.limit, 500);
      expect(names, ['n1', 'n2']);

      bundle.catchUp(3000);
      async.flushMicrotasks();
      expect(relay.fetches.last.since, 3000);
    });
  });

  test('catch-up hands a listener nothing from before it opened', () async {
    final relay = _FakeBundleRelay();
    final bundle = relay.bundle();
    final early = <String>[];
    final late = <String>[];
    NostrFilter replies() => NostrFilter(
      kinds: const [24211],
      tags: {
        '#p': [_me],
      },
      limit: 0,
    );
    relay.now = 1000;
    final first = bundle.subscribe(replies(), (e) => early.add(e.id));
    await pumpEventQueue();
    relay.opened.single.release();
    await first;
    relay.now = 2000;
    await bundle.subscribe(replies(), (e) => late.add(e.id));

    List<List<String>> p() => [
      ['p', _me],
    ];
    relay.fetchResult = [
      _event('old', 24211, p(), createdAt: 1500),
      _event('skewed', 24211, p(), createdAt: 1997),
      _event('new', 24211, p(), createdAt: 2100),
    ];
    await bundle.catchUp(100);
    expect(relay.fetches.last.since, 995, reason: 'earliest listener - skew');
    expect(early, ['old', 'skewed', 'new']);
    expect(late, ['skewed', 'new']);
  });

  test('reset drops every scope', () async {
    final relay = _FakeBundleRelay();
    final bundle = relay.bundle();
    final seen = <String>[];
    final subscribing = bundle.subscribe(
      NostrFilter(kinds: const [3], authors: [_me], limit: 0),
      (e) => seen.add(e.id),
    );
    await pumpEventQueue();
    relay.opened.single.release();
    await subscribing;
    bundle.reset();
    expect(relay.opened.single.closed, isTrue);
    relay.opened.single.onEvent(_event('c1', 3, const []));
    expect(seen, isEmpty);
  });
}

NostrFilter _doc(String dTag) => NostrFilter(
  kinds: const [30078],
  authors: [_me],
  tags: {
    '#d': [dTag],
  },
  limit: 0,
);

NostrEvent _event(
  String id,
  int kind,
  List<List<String>> tags, {
  int createdAt = 100,
}) => NostrEvent(
  id: id,
  pubkey: _me,
  createdAt: createdAt,
  kind: kind,
  tags: tags,
  content: '',
  sig: '',
);

class _FakeReq {
  _FakeReq(this.filter, this.onEvent, this.onClosed);

  final NostrFilter filter;
  final void Function(NostrEvent) onEvent;
  final void Function(String) onClosed;
  final _opened = Completer<void Function()>();
  bool closed = false;

  void release() => _opened.complete(() => closed = true);

  void fail() => _opened.completeError(Exception('fixture REQ failure'));
}

class _FakeBundleRelay {
  final opened = <_FakeReq>[];
  final fetches = <NostrFilter>[];
  var fetchFailures = 0;
  var fetchResult = <NostrEvent>[];
  var now = 0;

  MeLiveBundle bundle() => MeLiveBundle(
    nowSecs: () => now,
    open: (filter, onEvent, onClosed) {
      final req = _FakeReq(filter, onEvent, onClosed);
      opened.add(req);
      return req._opened.future;
    },
    fetch: (filter) async {
      fetches.add(filter);
      if (fetchFailures > 0) {
        fetchFailures--;
        throw Exception('fixture fetch failure');
      }
      return fetchResult;
    },
  );
}
