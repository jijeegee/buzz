import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/features/chats/chat_list.dart';
import 'package:kuzz/features/search/thread_name_search.dart';

ChatListRow _thread(String root, {bool closed = false}) => ChatListRow(
  isThread: true,
  key: root,
  at: 1,
  atUs: 1000000,
  channelId: 'general',
  channelType: 'stream',
  channelName: 'general',
  rootId: root,
  threadName: 'name $root',
  closed: closed,
);

void main() {
  test('the filter trims the query and refuses blank or long ones', () {
    expect(buildThreadSearchFilter('  launch '), {
      'thread_search': 1,
      'q': 'launch',
      'limit': threadNameSearchLimit,
    });
    expect(buildThreadSearchFilter('   '), isNull);
    expect(buildThreadSearchFilter('가' * threadNameSearchMaxChars), isNotNull);
    expect(buildThreadSearchFilter('x' * 101), isNull);
  });

  test('responses parse to thread rows; anything else is unsupported', () {
    final rows = parseThreadSearchResponse({
      'thread_search': 1,
      'rows': [
        {
          'type': 'thread',
          'key': 'r1',
          'root_id': 'r1',
          'at': 5,
          'at_us': 5000000,
          'channel_id': 'general',
          'channel_type': 'stream',
          'channel_name': 'general',
          'thread_name': 'Launch',
          'closed': true,
          'match': 0,
        },
        {'type': 'thread'},
      ],
    });
    expect(rows, hasLength(1));
    expect(rows.single.rootId, 'r1');
    expect(rows.single.threadName, 'Launch');
    expect(rows.single.closed, isTrue);

    expect(
      () => parseThreadSearchResponse(const []),
      throwsA(isA<ChatListUnsupportedException>()),
    );
    expect(
      () => parseThreadSearchResponse({'thread_search': 2, 'rows': []}),
      throwsA(isA<ChatListUnsupportedException>()),
    );
  });

  test('searched threads join the Chats rows once, Chats rows first', () {
    final chats = [_thread('a')];
    final merged = withSearchedThreads(chats, [
      _thread('a', closed: true),
      _thread('b', closed: true),
      _thread('b'),
    ]);
    expect([for (final row in merged) row.key], ['a', 'b']);
    expect(merged.first.closed, isFalse);
    expect(merged.last.closed, isTrue);
    expect(identical(withSearchedThreads(chats, const []), chats), isTrue);
  });
}
