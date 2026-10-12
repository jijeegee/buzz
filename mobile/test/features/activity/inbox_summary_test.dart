import 'package:buzz/features/activity/feed_item.dart';
import 'package:buzz/features/activity/inbox_item.dart';
import 'package:buzz/features/activity/inbox_summary.dart';
import 'package:buzz/shared/relay/relay.dart';
import 'package:flutter_test/flutter_test.dart';

const _room = '11111111-1111-4111-8111-111111111111';
const _dm = '22222222-2222-4222-8222-222222222222';
final _root = 'a' * 64;

Map<String, dynamic> _eventJson(
  String id,
  int createdAt, {
  String pubkey = 'other',
  String channel = _room,
  List<List<String>> extraTags = const [],
}) => {
  'id': id,
  'pubkey': pubkey,
  'created_at': createdAt,
  'kind': 9,
  'tags': [
    ['h', channel],
    ...extraTags,
  ],
  'content': id,
  'sig': null,
};

NostrEvent _event(
  String id,
  int createdAt, {
  String pubkey = 'other',
  String channel = _room,
  List<List<String>> extraTags = const [],
}) => NostrEvent.fromJson(
  _eventJson(
    id,
    createdAt,
    pubkey: pubkey,
    channel: channel,
    extraTags: extraTags,
  ),
);

List<List<String>> _replyTags(String rootId) => [
  ['e', rootId, '', 'root'],
  ['e', rootId, '', 'reply'],
];

Map<String, dynamic> _roomRow(
  String channel,
  int at, {
  int? unread,
  String type = 'stream',
  String latestId = 'room-latest',
}) => {
  'type': 'room',
  'channel_id': channel,
  'channel_type': type,
  'root_id': null,
  'at': at,
  'at_us': at * 1000000,
  'key': channel,
  'reply_count': null,
  'unread': unread,
  'unread_overflow': false,
  'latest': _eventJson(latestId, at, channel: channel),
  'root': null,
};

Map<String, dynamic> _threadRow(String rootId, int at, {int? unread}) => {
  'type': 'thread',
  'channel_id': _room,
  'channel_type': 'stream',
  'root_id': rootId,
  'at': at,
  'at_us': at * 1000000,
  'key': rootId,
  'reply_count': 2,
  'unread': unread,
  'unread_overflow': false,
  'latest': _eventJson('reply', at, extraTags: _replyTags(rootId)),
  'root': _eventJson(rootId, at - 50),
};

InboxSummaryState _state(
  List<Map<String, dynamic>> rows, {
  Map<String, int> sentRead = const {},
  Map<String, dynamic>? next,
}) {
  final page = parseInboxSummaryResponse({
    'inbox_summary': 1,
    'rows': rows,
    'next': next,
  });
  return InboxSummaryState(
    rows: page.rows,
    next: page.next,
    sentRead: sentRead,
  );
}

void main() {
  group('parseInboxSummaryResponse', () {
    test('reads rows, events, and the next cursor', () {
      final page = parseInboxSummaryResponse({
        'inbox_summary': 1,
        'rows': [
          _threadRow(_root, 200, unread: 3),
          _roomRow(_room, 100, unread: 0),
          {'type': 'room'},
        ],
        'next': {'at_us': 100000000, 'key': _room},
      });
      expect(page.rows, hasLength(2));
      final thread = page.rows.first;
      expect(thread.isThread, isTrue);
      expect(thread.rootId, _root);
      expect(thread.unread, 3);
      expect(thread.latest?.id, 'reply');
      expect(thread.root?.id, _root);
      expect(page.next, const InboxSummaryCursor(atUs: 100000000, key: _room));
    });

    test('rejects anything but a version-1 envelope', () {
      for (final response in [
        <Object?>[],
        {'inbox_summary': 2, 'rows': <Object?>[]},
        {'inbox_summary': 1},
        null,
      ]) {
        expect(
          () => parseInboxSummaryResponse(response),
          throwsA(isA<InboxSummaryUnsupportedException>()),
        );
      }
    });
  });

  test('the filter carries the cursor, markers, and a bounded limit', () {
    final filter = buildInboxSummaryFilter(
      limit: 500,
      before: const InboxSummaryCursor(atUs: 42, key: 'k'),
      read: const {'thread:ab': 7},
    );
    expect(filter['inbox_summary'], 1);
    expect(filter['limit'], 100);
    expect(filter['read'], {'thread:ab': 7});
    expect(filter['before'], {'at_us': 42, 'key': 'k'});
    expect(
      buildInboxSummaryFilter(limit: 50, before: null, read: const {}),
      isNot(contains('before')),
    );
  });

  test('read contexts leave out message markers', () {
    expect(
      inboxSummaryReadContexts({_room: 10, 'thread:$_root': 20, 'msg:x': 30}),
      {_room: 10, 'thread:$_root': 20},
    );
  });

  test('a later page appends only rows not already loaded', () {
    final current = _state(
      [_roomRow(_room, 300)],
      next: {'at_us': 300000000, 'key': _room},
    );
    final merged = mergeInboxSummaryPage(
      current,
      parseInboxSummaryResponse({
        'inbox_summary': 1,
        'rows': [_roomRow(_room, 300), _roomRow(_dm, 100, type: 'dm')],
        'next': null,
      }),
    );
    expect(merged.rows.map((row) => row.key), [_room, _dm]);
    expect(merged.next, isNull);
  });

  group('applyLiveEventToInboxSummary', () {
    test('moves only the affected row to the top and counts unread', () {
      final current = _state([
        _roomRow(_dm, 300, type: 'dm', unread: 0),
        _roomRow(_room, 100, unread: 2),
      ]);
      final next = applyLiveEventToInboxSummary(
        current,
        _event('new', 400),
        channelId: _room,
        channelType: 'stream',
        currentPubkey: 'me',
      );
      expect(next.rows.map((row) => row.key), [_room, _dm]);
      expect(next.rows.first.latest?.id, 'new');
      expect(next.rows.first.unread, 3);
      expect(identical(next.rows.last, current.rows.first), isTrue);
    });

    test('my own message moves the row without counting it', () {
      final next = applyLiveEventToInboxSummary(
        _state([_roomRow(_room, 100, unread: 1)]),
        _event('mine', 200, pubkey: 'me'),
        channelId: _room,
        channelType: 'stream',
        currentPubkey: 'me',
      );
      expect(next.rows.single.latest?.id, 'mine');
      expect(next.rows.single.unread, 1);
    });

    test('a reply patches its thread, adding one when missing', () {
      final current = _state([_roomRow(_room, 100)]);
      final next = applyLiveEventToInboxSummary(
        current,
        _event('reply-2', 300, extraTags: _replyTags(_root)),
        channelId: _room,
        channelType: 'stream',
        currentPubkey: 'me',
      );
      expect(next.rows.map((row) => row.key), [_root, _room]);
      expect(next.rows.first.isThread, isTrue);
      expect(next.rows.first.replyCount, 1);
      // The room's main timeline is untouched by a non-broadcast reply.
      expect(next.rows.last.latest?.id, 'room-latest');
    });

    test('a broadcast reply patches both its thread and the room', () {
      final next = applyLiveEventToInboxSummary(
        _state([_threadRow(_root, 100), _roomRow(_room, 50)]),
        _event(
          'broadcast',
          300,
          extraTags: [
            ..._replyTags(_root),
            ['broadcast', '1'],
          ],
        ),
        channelId: _room,
        channelType: 'stream',
        currentPubkey: 'me',
      );
      expect(next.rows.map((row) => row.latest?.id), [
        'broadcast',
        'broadcast',
      ]);
      expect(next.rows.firstWhere((row) => row.isThread).replyCount, 3);
    });

    test('duplicates, older events, and other kinds change nothing', () {
      final current = _state([_roomRow(_room, 100)]);
      for (final event in [
        _event('room-latest', 100),
        _event('older', 50),
        NostrEvent.fromJson({..._eventJson('reaction', 500), 'kind': 7}),
      ]) {
        expect(
          identical(
            applyLiveEventToInboxSummary(
              current,
              event,
              channelId: _room,
              channelType: 'stream',
              currentPubkey: 'me',
            ),
            current,
          ),
          isTrue,
        );
      }
    });
  });

  test('a deletion of a shown message or root needs a reload', () {
    final state = _state([_threadRow(_root, 200), _roomRow(_room, 100)]);
    expect(inboxSummaryShowsAny(state, ['room-latest']), isTrue);
    expect(inboxSummaryShowsAny(state, [_root]), isTrue);
    expect(inboxSummaryShowsAny(state, ['some-reaction']), isFalse);
  });

  test('feed items group a thread under its root and rooms by channel', () {
    final state = _state([
      _threadRow(_root, 200),
      _roomRow(_room, 100),
      _roomRow(_dm, 50, type: 'dm', latestId: 'dm-latest'),
    ]);
    final rows = collapseChannelMainRows(
      buildInboxItems(
        inboxSummaryFeedItems(state.rows),
        isDmChannel: (id) => id == _dm,
      ),
    );
    expect(rows.map((row) => row.conversationId), [
      _root,
      'channel:$_room',
      'dm:$_dm',
    ]);
    expect(
      rows.map((row) => row.conversationId),
      state.rows.map(inboxSummaryConversationId),
    );
  });

  group('server unread', () {
    InboxItem rowFor(InboxSummaryState state, String conversationId) =>
        collapseChannelMainRows(
          buildInboxItems(inboxSummaryFeedItems(state.rows)),
        ).firstWhere((row) => row.conversationId == conversationId);

    test('shows while the marker is the one the relay counted against', () {
      final state = _state(
        [_threadRow(_root, 200, unread: 4), _roomRow(_room, 100, unread: 7)],
        sentRead: {_room: 40, 'thread:$_root': 60},
      );
      final unread = inboxSummaryUnread(state);
      final markers = {_room: 40, 'thread:$_root': 60};
      expect(
        inboxRowServerUnread(
          rowFor(state, 'channel:$_room'),
          unread,
          isDone: false,
          markerOf: (id) => markers[id],
        )?.count,
        7,
      );
      expect(
        inboxRowServerUnread(
          rowFor(state, _root),
          unread,
          isDone: false,
          markerOf: (id) => markers[id],
        )?.count,
        4,
      );
    });

    test('clears as soon as the row is read locally', () {
      final state = _state(
        [_roomRow(_room, 100, unread: 7)],
        sentRead: {_room: 40},
      );
      final unread = inboxSummaryUnread(state);
      final row = rowFor(state, 'channel:$_room');
      // Marking read moves the marker past the counted one.
      expect(
        inboxRowServerUnread(
          row,
          unread,
          isDone: false,
          markerOf: (id) => id == _room ? 100 : null,
        ),
        isNull,
      );
      expect(
        inboxRowServerUnread(
          row,
          unread,
          isDone: true,
          markerOf: (id) => id == _room ? 40 : null,
        ),
        isNull,
      );
    });

    test('a live message from someone else raises the count', () {
      final state = applyLiveEventToInboxSummary(
        _state([_roomRow(_room, 100, unread: 1)], sentRead: {_room: 40}),
        _event('new', 300),
        channelId: _room,
        channelType: 'stream',
        currentPubkey: 'me',
      );
      final server = inboxSummaryUnread(state)['channel:$_room']!;
      expect(server.count, 2);
      expect(server.latestAt, 300);
    });
  });

  test('feed items carry plain activity for the inbox pipeline', () {
    final items = inboxSummaryFeedItems(_state([_roomRow(_room, 100)]).rows);
    expect(items.single, isA<FeedItem>());
    expect(items.single.category, 'activity');
    expect(items.single.channelId, _room);
  });
}
