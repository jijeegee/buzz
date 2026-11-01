import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/shared/relay/relay.dart';

class _Req {
  _Req(this.filters, this.onEvent, this.onGone);

  final List<NostrFilter> filters;
  final void Function(NostrEvent) onEvent;
  final void Function() onGone;
  final opened = Completer<void Function()>();
  bool closed = false;

  void open() => opened.complete(() => closed = true);
}

NostrEvent _event(int kind, String id) => NostrEvent(
  id: id.padLeft(64, '0'),
  pubkey: 'b' * 64,
  createdAt: 1,
  kind: kind,
  tags: const [],
  content: '',
  sig: '',
);

Future<void> _flush() async {
  for (var i = 0; i < 10; i++) {
    await Future<void>.delayed(Duration.zero);
  }
}

void main() {
  final fixture =
      jsonDecode(
            File('../test-fixtures/room-live-request.json').readAsStringSync(),
          )
          as Map<String, dynamic>;
  final room = fixture['room'] as String;

  late List<_Req> reqs;
  late RoomLiveRegistry registry;
  var hold = false;

  List<_Req> open() => reqs.where((req) => !req.closed).toList();

  setUp(() {
    reqs = [];
    hold = false;
    registry = RoomLiveRegistry((filters, onEvent, onGone) {
      final req = _Req(filters, onEvent, onGone);
      reqs.add(req);
      if (!hold) req.open();
      return req.opened.future;
    });
  });

  test("the REQ's filters match the shared fixture", () {
    expect([
      for (final filter in roomLiveRequestFilters(room)) filter.toJson(),
    ], fixture['filters']);
  });

  test('two listeners of one room share one REQ, each gets its kind', () async {
    final typing = <String>[];
    final goals = <String>[];
    registry.listen(room, 20002, (e) => typing.add(e.id));
    registry.listen(room, roomLiveGoalTreeKind, (e) => goals.add(e.id));
    await _flush();
    expect(reqs, hasLength(1));
    reqs.single.onEvent(_event(20002, 't'));
    reqs.single.onEvent(_event(40110, 'g'));
    expect(typing, ['t'.padLeft(64, '0')]);
    expect(goals, ['g'.padLeft(64, '0')]);
  });

  test('a rebuild in the same task keeps the REQ', () async {
    final stop = registry.listen(room, 20002, (_) {});
    await _flush();
    stop();
    registry.listen(room, 20002, (_) {});
    await _flush();
    expect(reqs, hasLength(1));
    expect(open(), hasLength(1));
  });

  test('the last listener leaving closes the REQ a microtask later', () async {
    final stopA = registry.listen(room, 20002, (_) {});
    final stopB = registry.listen(room, roomLiveGoalTreeKind, (_) {});
    await _flush();
    stopA();
    await _flush();
    expect(open(), hasLength(1));
    stopB();
    stopB();
    await _flush();
    expect(open(), isEmpty);
    expect(registry.openRooms, isEmpty);
  });

  test('stopping before the REQ is up closes it when it arrives', () async {
    hold = true;
    final stop = registry.listen(room, 20002, (_) {});
    stop();
    await _flush();
    reqs.single.open();
    await _flush();
    expect(reqs.single.closed, isTrue);
  });

  test('resync calls onResync once per listener', () async {
    var goals = 0;
    registry.listen(
      room,
      roomLiveGoalTreeKind,
      (_) {},
      onResync: () => goals++,
    );
    registry.listen(room, 20002, (_) {});
    await _flush();
    registry.resync();
    expect(goals, 1);
  });

  test('a terminal CLOSED calls onGone; a later listener reopens', () async {
    var gone = 0;
    registry.listen(room, 20002, (_) {}, onGone: () => gone++);
    await _flush();
    reqs.single.onGone();
    expect(gone, 1);
    expect(registry.openRooms, isEmpty);
    registry.listen(room, 20002, (_) {});
    await _flush();
    expect(reqs, hasLength(2));
  });

  test('only the open-room kinds ride the REQ', () {
    expect(() => registry.listen(room, 9, (_) {}), throwsArgumentError);
  });
}
