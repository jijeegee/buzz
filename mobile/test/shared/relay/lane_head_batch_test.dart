import 'dart:async';

import 'package:fake_async/fake_async.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/shared/relay/lane_head_batch.dart';
import 'package:kuzz/shared/relay/relay.dart';

final _me = 'a' * 64;

NostrFilter _head(String d, {String? author, int limit = 1}) => NostrFilter(
  kinds: const [30078],
  authors: [author ?? _me],
  tags: {
    '#d': [d],
  },
  limit: limit,
);

NostrEvent _event(String id, String d, int createdAt, {String? author}) =>
    NostrEvent(
      id: id,
      pubkey: author ?? _me,
      createdAt: createdAt,
      kind: 30078,
      tags: [
        ['d', d],
      ],
      content: '',
      sig: '',
    );

void main() {
  late List<({NostrFilter filter, Completer<List<NostrEvent>> result})> reads;

  LaneHeadBatcher makeBatcher() {
    reads = [];
    return LaneHeadBatcher(
      fetch: (filter, timeout) {
        final result = Completer<List<NostrEvent>>();
        reads.add((filter: filter, result: result));
        return result.future;
      },
    );
  }

  test('only single-lane own head reads qualify', () {
    expect(laneHeadDTagOf(_head('channel-mutes')), 'channel-mutes');
    expect(laneHeadDTagOf(_head('channel-mutes', limit: 2)), isNull);
    expect(
      laneHeadDTagOf(
        NostrFilter(
          kinds: const [30078],
          authors: [_me],
          tags: const {
            '#t': ['read-state'],
          },
          limit: 1,
        ),
      ),
      isNull,
    );
    expect(
      laneHeadDTagOf(
        NostrFilter(
          kinds: const [30078],
          authors: [_me],
          tags: const {
            '#d': ['channel-mutes'],
          },
          since: 5,
          limit: 1,
        ),
      ),
      isNull,
    );
    expect(
      laneHeadDTagOf(
        NostrFilter(
          kinds: const [30078],
          authors: [_me, 'b' * 64],
          tags: const {
            '#d': ['channel-mutes'],
          },
          limit: 1,
        ),
      ),
      isNull,
    );
  });

  test('reads within 50 ms share one REQ; each gets its newest head', () {
    fakeAsync((async) {
      final batcher = makeBatcher();
      List<NostrEvent>? mutes;
      List<NostrEvent>? stars;
      List<NostrEvent>? theme;
      batcher
          .fetch(_head('channel-mutes'), const Duration(seconds: 8))
          .then((v) => mutes = v);
      async.elapse(const Duration(milliseconds: 30));
      batcher
          .fetch(_head('channel-stars'), const Duration(seconds: 8))
          .then((v) => stars = v);
      batcher
          .fetch(_head('community-theme'), const Duration(seconds: 8))
          .then((v) => theme = v);
      async.elapse(const Duration(milliseconds: 19));
      expect(reads, isEmpty);
      async.elapse(const Duration(milliseconds: 1));
      expect(reads, hasLength(1));
      expect(reads.single.filter.toJson(), {
        'kinds': [30078],
        'limit': 3,
        'authors': [_me],
        '#d': ['channel-mutes', 'channel-stars', 'community-theme'],
      });
      reads.single.result.complete([
        _event('m-old', 'channel-mutes', 1),
        _event('m-new', 'channel-mutes', 2),
        _event('s', 'channel-stars', 1),
        _event('x', 'channel-stars', 9, author: 'b' * 64),
      ]);
      async.flushMicrotasks();
      expect(mutes!.map((e) => e.id), ['m-new']);
      expect(stars!.map((e) => e.id), ['s']);
      expect(theme, isEmpty);
    });
  });

  test('a failed read fails every lane in it', () {
    fakeAsync((async) {
      final batcher = makeBatcher();
      final errors = <Object>[];
      for (final d in ['channel-mutes', 'channel-stars']) {
        batcher.fetch(_head(d), const Duration(seconds: 8)).catchError((
          Object e,
        ) {
          errors.add(e);
          return <NostrEvent>[];
        });
      }
      async.elapse(laneHeadBatchWindow);
      reads.single.result.completeError(TimeoutException('slow'));
      async.flushMicrotasks();
      expect(errors, hasLength(2));
    });
  });

  test('reset fails reads still waiting for the window', () {
    fakeAsync((async) {
      final batcher = makeBatcher();
      Object? error;
      batcher
          .fetch(_head('channel-mutes'), const Duration(seconds: 8))
          .catchError((Object e) {
            error = e;
            return <NostrEvent>[];
          });
      batcher.reset(StateError('reset'));
      async.elapse(laneHeadBatchWindow);
      expect(error, isA<StateError>());
      expect(reads, isEmpty);
    });
  });
}
