import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/features/channels/channel_event_order.dart';
import 'package:kuzz/shared/relay/relay.dart';

void main() {
  test('order numbers decide when every event carries one', () {
    // Accepted in this order although the sender clocks disagree.
    final first = _event('first', createdAt: 30, sequence: 1);
    final second = _event('second', createdAt: 10, sequence: 2);
    expect(_ids(sortChannelTimeline([second, first])), ['first', 'second']);
    expect(_ids(sortThreadReplies([second, first])), ['first', 'second']);
    expect(_ids(sortThreadReplies([first, second], newestFirst: true)), [
      'second',
      'first',
    ]);
  });

  test('timestamps decide when any event has no order number', () {
    // Pairwise these three form a cycle (a<b by number, b<c and c<a by
    // time); one rule for the whole list keeps the sort consistent.
    final a = _event('a', createdAt: 30, sequence: 1);
    final b = _event('b', createdAt: 10, sequence: 2);
    final c = _event('c', createdAt: 20);
    expect(_ids(sortChannelTimeline([a, b, c])), ['b', 'c', 'a']);
    expect(_ids(sortChannelTimeline([c, b, a])), ['b', 'c', 'a']);
    expect(_ids(sortThreadReplies([a, c, b])), ['b', 'c', 'a']);
  });
}

List<String> _ids(List<NostrEvent> events) => [
  for (final event in events) event.id,
];

NostrEvent _event(String id, {required int createdAt, int? sequence}) =>
    NostrEvent(
      id: id,
      pubkey: 'alice',
      createdAt: createdAt,
      kind: EventKind.streamMessageV2,
      tags: const [],
      content: '',
      channelSequence: sequence,
    );
