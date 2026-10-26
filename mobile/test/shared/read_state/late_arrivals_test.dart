import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/shared/read_state/late_arrivals.dart';
import 'package:kuzz/shared/relay/relay.dart';

NostrEvent _event(String id, int createdAt, {String pubkey = 'other'}) =>
    NostrEvent(
      id: id,
      pubkey: pubkey,
      createdAt: createdAt,
      kind: 9,
      tags: const [],
      content: '',
    );

void main() {
  test('a message from someone else dated at or before the read is late', () {
    final seen = {'a'};
    final events = [
      _event('a', 10),
      _event('late', 8),
      _event('mine', 7, pubkey: 'ME'),
      _event('new', 12),
    ];
    expect(lateArrivals(events, seen, 10, 'me'), ['late']);
    // Each event is judged once.
    expect(lateArrivals(events, seen, 12, 'me'), isEmpty);
  });
}
