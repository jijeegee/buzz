import 'dart:async';

import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/shared/relay/history_then_live.dart';
import 'package:kuzz/shared/relay/relay.dart';

void main() {
  test('the fetch waits until the listener is live', () async {
    final rig = _Rig();
    final starting = rig.start();
    await pumpEventQueue();
    expect(rig.order, ['listen']);
    rig.listening.complete();
    final handle = await starting;
    expect(rig.order, ['listen', 'fetch']);
    rig.fetches.single.complete(const []);
    expect(await handle.firstFetch, isTrue);
  });

  test('an event between ready and the fetch result is applied once, '
      'history oldest first', () async {
    final rig = _Rig();
    final starting = rig.start();
    rig.listening.complete();
    final handle = await starting;
    rig.emit(_event('gap', 300));
    rig.fetches.single.complete([
      _event('new', 200),
      _event('gap', 300),
      _event('old', 100),
    ]);
    await handle.firstFetch;
    expect(rig.applied, ['gap', 'old', 'new']);
  });

  test('stop during the fetch applies nothing', () async {
    final rig = _Rig();
    final starting = rig.start();
    rig.listening.complete();
    final handle = await starting;
    handle.stop();
    rig.fetches.single.complete([_event('late', 100)]);
    expect(await handle.firstFetch, isFalse);
    rig.emit(_event('after-stop', 100));
    expect(rig.applied, isEmpty);
    expect(rig.stopped, 1);
  });

  test('a fetch error reaches onFetchError; a refetch can succeed', () async {
    final rig = _Rig();
    final errors = <Object>[];
    final starting = rig.start(onFetchError: errors.add);
    rig.listening.complete();
    final handle = await starting;
    rig.fetches.single.completeError(TimeoutException('timed out'));
    expect(await handle.firstFetch, isFalse);
    expect(errors, hasLength(1));
    expect(rig.applied, isEmpty, reason: 'a failure is not an empty success');

    final again = handle.refetch();
    await pumpEventQueue();
    rig.fetches.last.complete([_event('e1', 100)]);
    expect(await again, isTrue);
    expect(rig.applied, ['e1']);
  });

  test('a listen failure throws without fetching', () async {
    final rig = _Rig();
    final starting = rig.start();
    rig.listening.completeError(Exception('closed'));
    await expectLater(starting, throwsException);
    expect(rig.order, ['listen']);
  });
}

class _Rig {
  final order = <String>[];
  final applied = <String>[];
  final listening = Completer<void>();
  final fetches = <Completer<List<NostrEvent>>>[];
  void Function(NostrEvent)? _live;
  var stopped = 0;

  void emit(NostrEvent event) => _live!(event);

  Future<HistoryThenLive> start({void Function(Object)? onFetchError}) =>
      historyThenLive(
        listen: (apply) async {
          order.add('listen');
          _live = apply;
          await listening.future;
          return () => stopped++;
        },
        fetch: () {
          order.add('fetch');
          final next = Completer<List<NostrEvent>>();
          fetches.add(next);
          return next.future;
        },
        apply: (event) => applied.add(event.id),
        onFetchError: onFetchError,
      );
}

NostrEvent _event(String id, int createdAt) => NostrEvent(
  id: id,
  pubkey: 'a' * 64,
  createdAt: createdAt,
  kind: 44100,
  tags: const [],
  content: '',
  sig: '',
);
