import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/features/channels/thread_header_actions.dart';
import 'package:kuzz/features/channels/timeline_message.dart';

TimelineMessage _reply(int at, [List<List<String>> tags = const []]) =>
    TimelineMessage(
      id: 'r$at',
      pubkey: 'p',
      createdAt: at,
      content: '',
      tags: tags,
    );

void main() {
  final root = 'a' * 64;

  test('a thread is closed while its latest reply is a close marker', () {
    expect(isThreadClosed(const []), false);
    expect(isThreadClosed([_reply(1)]), false);
    expect(
      isThreadClosed([
        _reply(1),
        _reply(2, [
          [threadClosedTag, root],
        ]),
      ]),
      true,
    );
    // Any later reply reopens it, a reopen marker included.
    expect(
      isThreadClosed([
        _reply(3, [
          [threadReopenedTag, root],
        ]),
        _reply(2, [
          [threadClosedTag, root],
        ]),
      ]),
      false,
    );
  });
}
