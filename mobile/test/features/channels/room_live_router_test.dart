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
        _event('unlike', kind: EventKind.deletion, tags: [
          ['e', 'like-a'],
        ]),
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
      router.routeBundleEvent(
        _reaction('like-?', target: 'unknown'),
        {'a', 'b'},
      ),
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
}

NostrEvent _event(
  String id, {
  String? room,
  int kind = EventKind.streamMessage,
  List<List<String>>? tags,
}) => NostrEvent(
  id: id,
  pubkey: 'b' * 64,
  createdAt: 100,
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
