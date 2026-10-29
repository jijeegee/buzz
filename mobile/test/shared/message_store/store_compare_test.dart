import 'package:flutter_test/flutter_test.dart';

import 'package:kuzz/shared/message_store/store_compare.dart';
import 'package:kuzz/shared/relay/nostr_models.dart';

NostrEvent _row(String id, int createdAt) => NostrEvent(
  id: id,
  pubkey: 'p',
  createdAt: createdAt,
  kind: 9,
  tags: const [],
  content: '',
  sig: '',
);

void main() {
  test(
    'rows inside the shared range are compared; live and paged-off rows are not',
    () {
      final stored = [
        _row('a', 10),
        _row('b', 20),
        _row('gone', 25),
        _row('c', 30),
      ];
      final relay = [
        _row('b', 20),
        _row('late', 22),
        _row('c', 30),
        _row('live', 40),
      ];
      final difference = storeRelayDifference(stored, relay)!;
      expect(difference.compared, 3);
      expect(difference.missing, ['late']);
      expect(difference.extra, ['gone']);
    },
  );

  test('identical answers match; an empty side compares nothing', () {
    final rows = [_row('a', 1), _row('b', 2)];
    final difference = storeRelayDifference(rows, rows)!;
    expect(difference.missing, isEmpty);
    expect(difference.extra, isEmpty);
    expect(storeRelayDifference(const [], rows), isNull);
    expect(storeRelayDifference(rows, const []), isNull);
  });

  NostrEvent event(
    String id,
    int createdAt,
    int kind, [
    List<List<String>> tags = const [],
    String content = '',
  ]) => NostrEvent(
    id: id,
    pubkey: 'p',
    createdAt: createdAt,
    kind: kind,
    tags: tags,
    content: content,
    sig: '',
  );

  test('overlays and summaries of shared rows are compared', () {
    final stored = [
      event('e1', 50, 40003, [
        ['e', 'a'],
      ]),
      event('s1', 60, 39005, [
        ['e', 'a'],
      ], '{"reply_count":2}'),
      event('x', 70, 7, [
        ['e', 'other'],
      ]),
    ];
    final relay = [
      event('r1', 55, 7, [
        ['e', 'a'],
      ]),
      event('s2', 61, 39005, [
        ['e', 'a'],
      ], '{"reply_count":3}'),
    ];
    StoreRelayOverlayDifference compare(int fence) =>
        storeRelayOverlayDifference(
          stored,
          relay,
          {'a'},
          replyCountsOf(stored),
          replyCountsOf(relay),
          fence,
        );
    final difference = compare(100);
    expect(difference.overlaysMissing, ['r1']);
    expect(difference.overlaysExtra, ['e1']);
    expect(difference.summaries, ['a']);
    // Nothing newer than the store's newest event is compared.
    final fenced = compare(52);
    expect(fenced.overlaysMissing, isEmpty);
    expect(fenced.overlaysExtra, ['e1']);
    expect(fenced.summaries, isEmpty);
  });

  test('an overlay its own answer removes is not a difference', () {
    final stored = [
      event('re', 50, 7, [
        ['e', 'a'],
      ]),
      event('del', 51, 5, [
        ['e', 're'],
      ]),
    ];
    final difference = storeRelayOverlayDifference(
      stored,
      const [],
      {'a'},
      const {},
      const {},
      99,
    );
    expect(difference.overlaysExtra, isEmpty);
  });

  test('only reply counts of summaries are compared', () {
    final stored = [
      event('s1', 60, 39005, [
        ['e', 'a'],
      ], '{"reply_count":2,"participants":["x"]}'),
    ];
    final difference = storeRelayOverlayDifference(
      stored,
      const [],
      {'a'},
      replyCountsOf(stored),
      {'a': (count: 2, at: 60)},
      99,
    );
    expect(difference.summaries, isEmpty);
  });

  test('comparisons are tallied by stream and type', () {
    resetStoreComparisonTally();
    final rows = [event('a', 1, 9), event('b', 2, 9)];
    logStoreComparison('room', 'room', rows, rows);
    logStoreComparison('thread', 'thread', rows, [
      ...rows,
      event('r', 2, 7, [
        ['e', 'a'],
      ]),
    ]);
    expect(storeComparisonTally['room.matches'], 1);
    expect(storeComparisonTally['thread.differs'], 1);
    expect(storeComparisonTally['overlays'], 1);
    expect(storeComparisonTally['missing'], 0);
  });
}
