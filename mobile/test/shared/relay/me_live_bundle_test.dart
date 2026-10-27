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
    req.onEvent(_event('m1', 30078, [
      ['d', 'channel-mutes'],
    ]));
    req.onEvent(_event('s1', 30078, [
      ['d', 'channel-stars'],
    ]));
    req.onEvent(_event('m1', 30078, [
      ['d', 'channel-mutes'],
    ]));
    expect(mutes, ['m1']);
    expect(stars, ['s1']);
    expect(relay.fetches, isEmpty, reason: 'limit 0 asks for no history');
  });

  test('history a filter asks for is fetched for it alone', () async {
    final relay = _FakeBundleRelay()
      ..fetchResult = [
        _event('late', 44100, [
          ['p', _me],
        ], createdAt: 9),
        _event('early', 44100, [
          ['p', _me],
        ], createdAt: 8),
      ];
    final bundle = relay.bundle();
    final seen = <String>[];
    final filter = NostrFilter(
      kinds: const [44100, 44101],
      tags: {
        '#p': [_me],
      },
      limit: 50,
      since: 5,
    );
    final subscribing = bundle.subscribe(filter, (e) => seen.add(e.id));
    await pumpEventQueue();
    expect(relay.opened.single.filter.kinds, meAddressedKinds);
    relay.opened.single.release();
    await subscribing;

    expect(relay.fetches.single.since, 5);
    expect(relay.fetches.single.limit, 50);
    expect(seen, ['early', 'late']);
    relay.opened.single.onEvent(
      _event('late', 44100, [
        ['p', _me],
      ]),
    );
    expect(seen, ['early', 'late']);
  });

  test('the last listener leaving closes the REQ after a grace period', () {
    fakeAsync((async) {
      final relay = _FakeBundleRelay();
      final bundle = relay.bundle();
      void Function()? stop;
      bundle.subscribe(_doc('channel-sort'), (_) {}).then((s) => stop = s);
      async.flushMicrotasks();
      relay.opened.single.release();
      async.flushMicrotasks();

      stop!();
      void Function()? again;
      bundle.subscribe(_doc('channel-sort'), (_) {}).then((s) => again = s);
      async.elapse(const Duration(seconds: 2));
      expect(relay.opened, hasLength(1), reason: 'a rebuild keeps the REQ');
      expect(relay.opened.single.closed, isFalse);

      again!();
      async.elapse(const Duration(seconds: 2));
      expect(relay.opened.single.closed, isTrue);
    });
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

  MeLiveBundle bundle() => MeLiveBundle(
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
