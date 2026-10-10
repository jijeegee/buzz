import 'package:buzz/features/channels/thread_depth.dart';
import 'package:buzz/features/channels/timeline_message.dart';
import 'package:flutter_test/flutter_test.dart';

TimelineMessage _msg(String id, {String? parentId, String? rootId}) =>
    TimelineMessage(
      id: id,
      pubkey: 'alice',
      createdAt: 1,
      content: id,
      parentId: parentId,
      rootId: rootId,
    );

void main() {
  final head = _msg('head');
  final reply = _msg('reply', parentId: 'head', rootId: 'head');
  final nested = _msg('nested', parentId: 'reply', rootId: 'head');
  final deeper = _msg('deeper', parentId: 'nested', rootId: 'head');

  test('threads are one level deep', () {
    expect(maxThreadDepth, 1);
  });

  test('only messages shallower than the limit can be replied to', () {
    expect(canReplyInThread(0), isTrue);
    expect(canReplyInThread(maxThreadDepth - 1), isTrue);
    expect(canReplyInThread(maxThreadDepth), isFalse);
    expect(canReplyInThread(maxThreadDepth + 1), isFalse);
  });

  test('depth counts parents up to the thread head', () {
    final all = [head, reply, nested, deeper];
    expect(threadDepthOf(head, all), 0);
    expect(threadDepthOf(reply, all), 1);
    expect(threadDepthOf(nested, all), 2);
    expect(threadDepthOf(deeper, all), 3);
  });

  test('a nested reply with an unloaded parent counts as depth 2', () {
    expect(threadDepthOf(nested), 2);
    expect(threadDepthOf(deeper, [head]), 2);
  });

  test('a parent cycle terminates', () {
    final a = _msg('a', parentId: 'b', rootId: 'root');
    final b = _msg('b', parentId: 'a', rootId: 'root');
    expect(threadDepthOf(a, [a, b]), 2);
  });
}
