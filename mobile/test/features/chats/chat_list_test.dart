import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/features/chats/chat_list.dart';
import 'package:kuzz/shared/relay/relay.dart';

const _channel = '11111111-1111-4111-8111-111111111111';
const _other = '22222222-2222-4222-8222-222222222222';
final _me = 'a' * 64;
final _them = 'b' * 64;
final _root = 'c' * 64;

NostrEvent _event({
  required String id,
  required int at,
  String? pubkey,
  List<List<String>> tags = const [],
  String content = 'hi',
  int kind = 9,
}) => NostrEvent(
  id: id,
  pubkey: pubkey ?? _them,
  createdAt: at,
  kind: kind,
  tags: tags,
  content: content,
  sig: '',
);

ChatListRow _room(
  String channelId,
  int at, {
  int unread = 0,
  bool unreadMark = false,
  NostrEvent? latest,
}) => ChatListRow(
  isThread: false,
  key: channelId,
  at: at,
  atUs: at * 1000000,
  channelId: channelId,
  channelType: 'stream',
  channelName: 'general',
  unread: unread,
  unreadMark: unreadMark,
  latest: latest,
);

ChatListRow _thread(
  String channelId,
  String rootId,
  int at, {
  int unread = 0,
  String? name,
  NostrEvent? root,
}) => ChatListRow(
  isThread: true,
  key: rootId,
  at: at,
  atUs: at * 1000000,
  channelId: channelId,
  channelType: 'stream',
  channelName: 'general',
  rootId: rootId,
  threadName: name,
  unread: unread,
  root: root,
);

void main() {
  group('chat_list request and response', () {
    test('builds the version-1 filter and clamps the limit', () {
      expect(
        buildChatListFilter(
          limit: 500,
          before: const ChatListCursor(atUs: 5, key: 'k'),
        ),
        {
          'chat_list': 1,
          'limit': 100,
          'before': {'at_us': 5, 'key': 'k'},
        },
      );
    });

    test('parses rows and skips malformed ones', () {
      final page = parseChatListResponse({
        'chat_list': 1,
        'rows': [
          {
            'type': 'thread',
            'key': _root,
            'at': 10,
            'at_us': 10000000,
            'channel_id': _channel,
            'channel_type': 'stream',
            'channel_name': 'general',
            'root_id': _root,
            'thread_name': 'Plan',
            'unread': 3,
            'unread_mentions': 1,
          },
          {'type': 'room', 'key': _channel},
        ],
        'next': {'at_us': 10000000, 'key': _root},
      });
      expect(page.rows, hasLength(1));
      final row = page.rows.single;
      expect(row.isThread, isTrue);
      expect(row.threadName, 'Plan');
      expect(row.unread, 3);
      expect(row.unreadMentions, 1);
      expect(page.next?.key, _root);
      expect(ChatListRow.fromJson(row.toJson())?.unread, 3);
    });

    test('anything but the version-1 envelope is unsupported', () {
      expect(
        () => parseChatListResponse({'rows': []}),
        throwsA(isA<ChatListUnsupportedException>()),
      );
      expect(
        isChatStateDisabledError(
          RelayException(404, '{"error":"chat_state_disabled"}'),
        ),
        isTrue,
      );
      expect(isChatStateDisabledError(RelayException(500, 'x')), isFalse);
    });
  });

  group('ordering and grouping', () {
    test('starred rooms come first, then newest activity', () {
      final rows = [
        _room(_channel, 10),
        _room(_other, 30),
        _thread(_channel, _root, 20),
      ];
      expect(orderChatRows(rows, {}).map((row) => row.key), [
        _other,
        _root,
        _channel,
      ]);
      expect(orderChatRows(rows, {_channel}).map((row) => row.key), [
        _channel,
        _other,
        _root,
      ]);
    });

    test('By channel shows two threads and folds the rest', () {
      final threads = [
        for (var i = 0; i < 4; i++)
          _thread(_channel, '$i' * 64, 40 - i, unread: i),
      ];
      final ordered = orderChatRows([...threads, _room(_other, 50)], {});
      final folded = groupChatRowsByChannel(
        ordered,
        expandedChannelIds: {},
        unreadOf: (row) => row.unread,
      );
      expect(folded, hasLength(5));
      expect((folded[0] as ChatGroupRoom).channelId, _other);
      final header = folded[1] as ChatGroupRoom;
      expect(header.channelId, _channel);
      expect(header.row, isNull);
      expect((folded[2] as ChatGroupThread).row.key, '0' * 64);
      expect((folded[3] as ChatGroupThread).row.key, '1' * 64);
      final more = folded[4] as ChatGroupMore;
      expect(more.hidden, 2);
      expect(more.hiddenUnread, 2 + 3);
      expect(more.expanded, isFalse);

      final expanded = groupChatRowsByChannel(
        ordered,
        expandedChannelIds: {_channel},
        unreadOf: (row) => row.unread,
      );
      expect(expanded.whereType<ChatGroupThread>(), hasLength(4));
      expect((expanded.last as ChatGroupMore).expanded, isTrue);
    });

    test('thread title falls back to the root first line', () {
      expect(
        chatThreadTitle(_thread(_channel, _root, 1, name: ' Plan ')),
        'Plan',
      );
      expect(
        chatThreadTitle(
          _thread(
            _channel,
            _root,
            1,
            root: _event(id: _root, at: 1, content: '\n first \nsecond'),
          ),
        ),
        'first',
      );
      expect(chatThreadTitle(_thread(_channel, _root, 1)), 'Thread');
    });

    test('an attachment-only preview reads Photo', () {
      expect(
        chatRowPreviewText(
          _event(
            id: 'x',
            at: 1,
            content: ' ',
            tags: [
              ['imeta', 'url https://example.com/a.png'],
            ],
          ),
        ),
        'Photo',
      );
    });
  });

  group('unread', () {
    test('this device having read past the latest hides the count', () {
      final row = _room(
        _channel,
        10,
        unread: 4,
        latest: _event(id: 'l', at: 10),
      );
      expect(chatRowUnread(row, null).count, 4);
      expect(chatRowUnread(row, 9).isUnread, isTrue);
      expect(chatRowUnread(row, 10).isUnread, isFalse);
    });

    test('a row marked unread stays unread', () {
      final row = _room(
        _channel,
        10,
        unreadMark: true,
        latest: _event(id: 'l', at: 10),
      );
      final unread = chatRowUnread(row, 20);
      expect(unread.isUnread, isTrue);
      expect(unread.marked, isTrue);
    });

    test('counts past the cap read 99+', () {
      expect(formatChatCount(5, false), '5');
      expect(formatChatCount(99, true), '99+');
    });
  });

  group('live events', () {
    test('another person moves the room up and adds unread', () {
      final state = ChatListState(
        rows: [
          _room(_other, 20),
          _room(_channel, 10, latest: _event(id: 'old', at: 10)),
        ],
        next: null,
      );
      final result = applyLiveEventToChatList(
        state,
        _event(
          id: 'new',
          at: 30,
          tags: [
            ['p', _me],
          ],
        ),
        channelId: _channel,
        currentPubkey: _me,
      );
      expect(result.refetch, isFalse);
      final top = result.state.rows.first;
      expect(top.key, _channel);
      expect(top.unread, 1);
      expect(top.unreadMentions, 1);
    });

    test('my own message clears the row', () {
      final state = ChatListState(
        rows: [_room(_channel, 10, unread: 5, unreadMark: true)],
        next: null,
      );
      final row = applyLiveEventToChatList(
        state,
        _event(id: 'mine', at: 30, pubkey: _me),
        channelId: _channel,
        currentPubkey: _me,
      ).state.rows.single;
      expect(row.unread, 0);
      expect(row.unreadMark, isFalse);
    });

    test('a reply moves its thread; an unknown room asks for a refetch', () {
      final state = ChatListState(
        rows: [_room(_channel, 10), _thread(_channel, _root, 5)],
        next: null,
      );
      final reply = applyLiveEventToChatList(
        state,
        _event(
          id: 'r',
          at: 40,
          tags: [
            ['e', _root, '', 'root'],
            ['e', _root, '', 'reply'],
          ],
        ),
        channelId: _channel,
        currentPubkey: _me,
      );
      expect(reply.state.rows.first.key, _root);
      expect(reply.state.rows.first.unread, 1);
      expect(reply.state.rows.last.unread, 0);

      final unknown = applyLiveEventToChatList(
        state,
        _event(id: 'u', at: 40),
        channelId: _other,
        currentPubkey: _me,
      );
      expect(unknown.refetch, isTrue);
    });

    test('a row read on this device counts new messages from zero', () {
      final state = ChatListState(
        rows: [_room(_channel, 10, unread: 5, latest: _event(id: 'l', at: 10))],
        next: null,
      );
      final row = applyLiveEventToChatList(
        state,
        _event(id: 'n', at: 20),
        channelId: _channel,
        currentPubkey: _me,
        readAtOf: (_) => 10,
      ).state.rows.single;
      expect(row.unread, 1);
    });

    test('a replayed event from the fetched second is not counted twice', () {
      final state = ChatListState(
        rows: [_room(_channel, 10, unread: 1, latest: _event(id: 'l', at: 10))],
        next: null,
      );
      final replayed = applyLiveEventToChatList(
        state,
        _event(id: 'same-second', at: 10),
        channelId: _channel,
        currentPubkey: _me,
        replay: true,
      );
      expect(replayed.state.rows.single.unread, 1);
      final live = applyLiveEventToChatList(
        state,
        _event(id: 'same-second', at: 10),
        channelId: _channel,
        currentPubkey: _me,
      );
      expect(live.state.rows.single.unread, 2);
    });

    test('non-message kinds are ignored', () {
      final state = ChatListState(rows: [_room(_channel, 10)], next: null);
      final result = applyLiveEventToChatList(
        state,
        _event(id: 'k', at: 40, kind: 7),
        channelId: _channel,
        currentPubkey: _me,
      );
      expect(identical(result.state, state), isTrue);
      expect(result.refetch, isFalse);
    });
  });

  group('paging', () {
    test('a refetched first page keeps older loaded rows', () {
      final current = ChatListState(
        rows: [_room(_channel, 30), _room(_other, 10)],
        next: const ChatListCursor(atUs: 10000000, key: _other),
      );
      final page = ChatListPage(
        rows: [_room(_channel, 40)],
        next: const ChatListCursor(atUs: 40000000, key: _channel),
      );
      final merged = mergeRefetchedFirstPage(current, page);
      expect(merged.rows.map((row) => row.key), [_channel, _other]);
      expect(merged.rows.first.at, 40);
      expect(merged.next?.key, _other);
    });

    test('a cached list is replaced, not merged', () {
      final cached = ChatListState(
        rows: [_room(_other, 10)],
        next: null,
        fromCache: true,
      );
      final page = ChatListPage(
        rows: [_room(_channel, 40)],
        next: const ChatListCursor(atUs: 40000000, key: _channel),
      );
      expect(mergeRefetchedFirstPage(cached, page).rows.map((row) => row.key), [
        _channel,
      ]);
    });

    test('a later page appends without duplicates', () {
      final current = ChatListState(rows: [_room(_channel, 30)], next: null);
      final merged = mergeChatListPage(
        current,
        ChatListPage(
          rows: [_room(_channel, 30), _room(_other, 10)],
          next: null,
        ),
      );
      expect(merged.rows.map((row) => row.key), [_channel, _other]);
    });
  });
}
