import 'dart:async';

import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/features/channels/room_live_router.dart';
import 'package:kuzz/shared/relay/relay.dart';

void main() {
  test('a covered room gets bundle events once, with no REQ', () async {
    final relay = _FakeRoomRelay();
    final router = RoomLiveRouter(relay.open)..setCoveredRooms({'a', 'b'});
    final seen = <String>[];
    final stop = await router.listenRoom('a', (event) => seen.add(event.id));

    expect(relay.opened, isEmpty);
    expect(router.routeBundleEvent(_event('row', room: 'a'), {'a', 'b'}), 'a');
    // An overlapping chunk carries the same event again.
    expect(router.routeBundleEvent(_event('row', room: 'a'), {'a'}), 'a');
    expect(
      router.routeBundleEvent(
        _event('summary', room: 'a', kind: EventKind.channelThreadSummary),
        {'a', 'b'},
      ),
      'a',
    );
    router.routeBundleEvent(_event('other', room: 'b'), {'a', 'b'});
    expect(seen, ['row', 'summary']);
    stop();
  });

  test('an uncovered room rides its own REQ until covered', () async {
    final relay = _FakeRoomRelay();
    final router = RoomLiveRouter(relay.open);
    final seen = <String>[];
    final stop = await router.listenRoom('a', (event) => seen.add(event.id));

    final own = relay.opened.single;
    expect(own.filter.kinds, roomLiveEventKinds);
    expect(own.filter.tags['#h'], ['a']);
    expect(own.filter.limit, 0);
    expect(own.filter.since, isNull);

    own.onEvent(_event('early', room: 'a'));
    router.setCoveredRooms({'a'});
    expect(own.closed, isTrue, reason: 'the bundle took over');
    // The bundle redelivers what the room subscription already gave.
    router.routeBundleEvent(_event('early', room: 'a'), {'a'});
    router.routeBundleEvent(_event('later', room: 'a'), {'a'});
    expect(seen, ['early', 'later']);

    router.setCoveredRooms({});
    expect(relay.opened, hasLength(2), reason: 'coverage lost: reopen');
    stop();
    await Future<void>.delayed(Duration.zero);
    expect(relay.opened.last.closed, isTrue);
  });

  test('h-less reactions find their room', () async {
    final relay = _FakeRoomRelay();
    final router = RoomLiveRouter(relay.open)..setCoveredRooms({'a', 'b', 'c'});
    final inA = <String>[];
    final inB = <String>[];
    final stopA = await router.listenRoom('a', (event) => inA.add(event.id));
    final stopB = await router.listenRoom(
      'b',
      (event) => inB.add(event.id),
      holdsEvent: (id) => id == 'paged-b',
    );

    router.routeBundleEvent(_event('row-a', room: 'a'), {'a', 'b'});
    expect(
      router.routeBundleEvent(_reaction('like-a', target: 'row-a'), {'a', 'b'}),
      'a',
    );
    // Removing the reaction references the reaction itself.
    expect(
      router.routeBundleEvent(
        _event(
          'unlike',
          kind: EventKind.deletion,
          tags: [
            ['e', 'like-a'],
          ],
        ),
        {'a', 'b'},
      ),
      'a',
    );
    expect(
      router.routeBundleEvent(_reaction('like-b', target: 'paged-b'), {
        'a',
        'b',
      }),
      'b',
    );
    expect(
      router.routeBundleEvent(_reaction('like-c', target: 'unknown'), {'c'}),
      'c',
      reason: 'a one-room chunk names its room',
    );
    expect(
      router.routeBundleEvent(_reaction('like-?', target: 'unknown'), {
        'a',
        'b',
      }),
      isNull,
    );
    expect(inA, ['row-a', 'like-a', 'unlike']);
    expect(inB, ['like-b']);
    stopA();
    stopB();
  });

  test('another room\'s event on a room REQ reaches that room', () async {
    final relay = _FakeRoomRelay();
    final router = RoomLiveRouter(relay.open);
    final inA = <String>[];
    final inB = <String>[];
    final stopA = await router.listenRoom('a', (event) => inA.add(event.id));
    final stopB = await router.listenRoom('b', (event) => inB.add(event.id));

    final b = _event('for-b', room: 'b');
    for (final sub in relay.opened) {
      sub.onEvent(b);
    }
    expect(inA, isEmpty);
    expect(inB, ['for-b']);
    stopA();
    stopB();
  });

  test('a failed room REQ settles and keeps the listener', () async {
    final relay = _FakeRoomRelay(fail: true);
    final router = RoomLiveRouter(relay.open);
    final seen = <String>[];
    final stop = await router.listenRoom('a', (event) => seen.add(event.id));

    router.setCoveredRooms({'a'});
    router.routeBundleEvent(_event('row', room: 'a'), {'a'});
    expect(seen, ['row']);
    stop();
  });

  test('thread names reach metadata listeners only, once', () async {
    final relay = _FakeRoomRelay();
    final router = RoomLiveRouter(relay.open)..setCoveredRooms({'a'});
    final inRoom = <String>[];
    final names = <String>[];
    final stopRoom = await router.listenRoom('a', (e) => inRoom.add(e.id));
    final stopNames = router.listenMetadata((e) => names.add(e.id));
    final rename = _event('rename', room: 'a', kind: EventKind.threadName);

    expect(router.routeBundleEvent(rename, {'a'}), 'a');
    router.routeBundleEvent(rename, {'a'});
    expect(names, ['rename']);
    expect(inRoom, isEmpty);
    expect(reachesAllRoomListeners(rename), isFalse);
    stopNames();
    stopRoom();
  });

  test("a browsed room's own REQ carries thread names", () async {
    final relay = _FakeRoomRelay();
    final router = RoomLiveRouter(relay.open);
    final names = <String>[];
    router.listenMetadata((e) => names.add(e.id));
    final stop = await router.listenRoom('x', (_) {});
    expect(relay.opened.single.filter.kinds, contains(EventKind.threadName));
    relay.opened.single.onEvent(
      _event('rename', room: 'x', kind: EventKind.threadName),
    );
    expect(names, ['rename']);
    stop();
  });

  group('whenRoomCovered', () {
    test('completes when the bundle covers the room', () async {
      final router = RoomLiveRouter(_FakeRoomRelay().open)
        ..setBundleRooms({'a'});
      var done = false;
      unawaited(router.whenRoomCovered('a').then((_) => done = true));
      await Future<void>.delayed(Duration.zero);
      expect(done, isFalse);
      router.setCoveredRooms({'a'});
      await Future<void>.delayed(Duration.zero);
      expect(done, isTrue);
    });

    test('a room nothing will carry completes at once', () async {
      final router = RoomLiveRouter(_FakeRoomRelay().open)
        ..setBundleRooms({'a'});
      var done = false;
      unawaited(router.whenRoomCovered('archived').then((_) => done = true));
      await Future<void>.delayed(Duration.zero);
      expect(done, isTrue);
    });

    test('a listened room completes once its own REQ is up', () async {
      final router = RoomLiveRouter(_FakeRoomRelay().open)
        ..setBundleRooms({'a'});
      final stop = await router.listenRoom('browsed', (_) {});
      var done = false;
      unawaited(router.whenRoomCovered('browsed').then((_) => done = true));
      await Future<void>.delayed(Duration.zero);
      expect(done, isTrue);
      stop();
    });

    test('cold start (rooms unknown) completes at the wait limit', () async {
      final router = RoomLiveRouter(
        _FakeRoomRelay().open,
        coverageWaitLimit: const Duration(milliseconds: 30),
      );
      final watch = Stopwatch()..start();
      await router.whenRoomCovered('a');
      expect(watch.elapsedMilliseconds, greaterThanOrEqualTo(25));
    });

    test('the default wait is 1.5 s', () {
      expect(roomCoverageWaitLimit, const Duration(milliseconds: 1500));
    });

    test('a quiet room waits for its chunk', () async {
      final router = RoomLiveRouter(_FakeRoomRelay().open)
        ..setQuietRooms({'q'})
        ..setBundleRooms({'a', 'q'});
      var done = false;
      unawaited(router.whenRoomCovered('q').then((_) => done = true));
      await Future<void>.delayed(Duration.zero);
      expect(done, isFalse);
      router.setCoveredRooms({'a', 'q'});
      await Future<void>.delayed(Duration.zero);
      expect(done, isTrue);
    });
  });

  group('quiet rooms', () {
    test("a quiet room's events reach quiet listeners with their room; "
        'summaries and thread names do not', () {
      final router = RoomLiveRouter(_FakeRoomRelay().open)
        ..setQuietRooms({'q'})
        ..setBundleRooms({'a', 'q'})
        ..setCoveredRooms({'a', 'q'});
      final quiet = <(String, String)>[];
      final metadata = <String>[];
      router
        ..listenQuiet((event, roomId) => quiet.add((event.id, roomId)))
        ..listenMetadata((event) => metadata.add(event.id));
      const chunk = {'a', 'q'};
      router
        ..routeBundleEvent(_event('row-q', room: 'q'), chunk)
        ..routeBundleEvent(_event('row-q', room: 'q'), chunk)
        // An h-less reaction in a quiet room reaches the quiet listener.
        ..routeBundleEvent(_reaction('like', target: 'row-q'), chunk)
        ..routeBundleEvent(
          _event('sum', room: 'q', kind: EventKind.channelThreadSummary),
          chunk,
        )
        ..routeBundleEvent(
          _event('name', room: 'q', kind: EventKind.threadName),
          chunk,
        )
        ..routeBundleEvent(_event('row-a', room: 'a'), chunk);
      expect(quiet, [('row-q', 'q'), ('like', 'q')]);
      expect(metadata, ['name']);
      expect(router.isQuietRoom('q'), isTrue);
      expect(router.isQuietRoom('a'), isFalse);
    });

    test('a passive room listener opens no REQ and hears the bundle', () {
      final relay = _FakeRoomRelay();
      final router = RoomLiveRouter(relay.open)..setBundleRooms({'a'});
      final heard = <String>[];
      final stop = router.listenRoomEvents('b', (e) => heard.add(e.id));
      expect(relay.opened, isEmpty);
      router.routeBundleEvent(
        _event('joined', room: 'b', kind: EventKind.systemMessage),
        {'a', 'b'},
      );
      stop();
      router.routeBundleEvent(_event('later', room: 'b'), {'a', 'b'});
      expect(heard, ['joined']);
      expect(relay.opened, isEmpty);
    });

    test("backfill delivers a quiet room's missed events once", () {
      var now = 1000;
      final router =
          RoomLiveRouter(_FakeRoomRelay().open, nowSeconds: () => now)
            ..setQuietRooms({'q'})
            ..setBundleRooms({'a', 'q'});
      now = 2000;
      final quiet = <String>[];
      router
        ..listenQuiet((event, _) => quiet.add(event.id))
        ..routeBundleEvent(_event('live', room: 'q', createdAt: 1500), {
          'a',
          'q',
        });
      final delivered = router.routeQuietBackfill([
        _event('live', room: 'q', createdAt: 1500),
        _event('missed', room: 'q', createdAt: 1600),
        _event('missed', room: 'q', createdAt: 1600),
        _event('listed', room: 'a', createdAt: 1600),
      ]);
      expect(delivered.map((e) => e.id), ['missed']);
      expect(quiet, ['live', 'missed']);
    });

    test("backfill never replays a room's past from before it was carried", () {
      var now = 10000;
      final router = RoomLiveRouter(
        _FakeRoomRelay().open,
        nowSeconds: () => now,
      )..setBundleRooms({'a'});
      now = 20000;
      // Hidden now: the DM enters the bundle as a quiet room directly.
      router
        ..setQuietRooms({'q'})
        ..setBundleRooms({'a', 'q'});
      expect(router.carriedSince('q'), 20000);
      expect(router.carriedSince('a'), 10000);
      final delivered = router.routeQuietBackfill([
        _event('ten-minutes-old', room: 'q', createdAt: 20000 - 600),
        _event('skewed', room: 'q', createdAt: 20000 - 5),
      ]);
      expect(delivered.map((e) => e.id), ['skewed']);
    });
  });

  group('chunk ready ticks', () {
    test(
      'a ready after the tick settles the wait; one before it does not',
      () async {
        final router = RoomLiveRouter(_FakeRoomRelay().open)
          ..markRoomsReady({'a', 'b'});
        final tick = router.readyTick;
        final ready = router.whenRoomsReadySince({'a', 'b'}, tick);
        var settled = false;
        unawaited(ready.then((_) => settled = true));
        router.markRoomsReady({'a'});
        await Future<void>.delayed(Duration.zero);
        expect(settled, isFalse, reason: 'b was not ready again yet');
        router.markRoomsReady({'b'});
        expect(await ready, isTrue);
        expect(await router.whenRoomsReadySince({'a'}, tick), isTrue);
      },
    );

    test('the wait is bounded and ends with the router', () async {
      final router = RoomLiveRouter(_FakeRoomRelay().open);
      expect(
        await router.whenRoomsReadySince(
          {'a'},
          router.readyTick,
          limit: const Duration(milliseconds: 10),
        ),
        isFalse,
      );
      final pending = router.whenRoomsReadySince({'a'}, router.readyTick);
      router.dispose();
      expect(await pending, isFalse);
    });
  });
}

NostrEvent _event(
  String id, {
  String? room,
  int kind = EventKind.streamMessage,
  List<List<String>>? tags,
  int createdAt = 100,
}) => NostrEvent(
  id: id,
  pubkey: 'b' * 64,
  createdAt: createdAt,
  kind: kind,
  tags:
      tags ??
      [
        if (room != null) ['h', room],
      ],
  content: id,
  sig: '',
);

NostrEvent _reaction(String id, {required String target}) => _event(
  id,
  kind: EventKind.reaction,
  tags: [
    ['e', target],
  ],
);

class _FakeRoomSubscription {
  _FakeRoomSubscription(this.filter, this.onEvent);

  final NostrFilter filter;
  final void Function(NostrEvent) onEvent;
  bool closed = false;
}

class _FakeRoomRelay {
  _FakeRoomRelay({this.fail = false});

  final bool fail;
  final opened = <_FakeRoomSubscription>[];

  Future<void Function()> open(
    NostrFilter filter,
    void Function(NostrEvent) onEvent,
  ) async {
    final sub = _FakeRoomSubscription(filter, onEvent);
    opened.add(sub);
    if (fail) throw Exception('fixture subscription failure');
    return () => sub.closed = true;
  }
}
