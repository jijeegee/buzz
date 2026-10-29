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
}
