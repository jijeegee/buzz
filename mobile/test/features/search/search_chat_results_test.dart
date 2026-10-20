import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/features/channels/channel.dart';
import 'package:kuzz/features/chats/chat_list.dart';
import 'package:kuzz/features/search/search_chat_results.dart';
import 'package:kuzz/features/search/search_provider.dart';

Channel _channel(
  String id,
  String name, {
  bool isMember = true,
  String type = 'stream',
  String description = '',
  List<String> participantPubkeys = const [],
}) => Channel(
  id: id,
  name: name,
  channelType: type,
  visibility: 'open',
  description: description,
  createdBy: 'test',
  createdAt: DateTime(2025),
  memberCount: 2,
  isMember: isMember,
  participantPubkeys: participantPubkeys,
);

ChatListRow _room(String channelId, int at, {String type = 'stream'}) =>
    ChatListRow(
      isThread: false,
      key: channelId,
      at: at,
      atUs: at * 1000000,
      channelId: channelId,
      channelType: type,
      channelName: channelId,
    );

ChatListRow _thread(
  String root,
  String title,
  int at, {
  String channelId = 'general',
  String type = 'stream',
}) => ChatListRow(
  isThread: true,
  key: root,
  at: at,
  atUs: at * 1000000,
  channelId: channelId,
  channelType: type,
  channelName: channelId,
  rootId: root,
  threadName: title,
);

SearchChatResults _build(
  String query, {
  List<ChatListRow> rows = const [],
  List<Channel> channels = const [],
  Map<String, String> dmLabels = const {},
}) => buildSearchChatResults(
  query: query,
  rows: rows,
  channels: channels,
  dmLabel: (channel) => dmLabels[channel.id] ?? channel.name,
);

void main() {
  group('searchMatchRank', () {
    test('ranks exact, then starts with, then word start, then middle', () {
      expect(searchMatchRank('검색', '검색'), 0);
      expect(searchMatchRank('검색창 개선', '검색'), 1);
      expect(searchMatchRank('채널 검색 및 초대', '검색'), 2);
      expect(searchMatchRank('design-review', 'review'), 2);
      expect(searchMatchRank('통합검색', '검색'), 3);
      expect(searchMatchRank('General', 'GEN'), 1);
      expect(searchMatchRank('general', 'xyz'), isNull);
      expect(searchMatchRank('', '검색'), isNull);
      expect(searchMatchRank('general', '  '), isNull);
    });
  });

  group('buildSearchChatResults', () {
    test('finds a thread by a word in the middle of its title', () {
      final results = _build(
        '검색',
        rows: [
          _room('general', 100),
          _thread('root-1', '채널 검색 및 초대', 90),
          _thread('root-2', 'Release notes', 95),
        ],
        channels: [_channel('general', 'general')],
      );

      expect(results.groups.map((r) => r.title), ['채널 검색 및 초대']);
      final thread = results.groups.single;
      expect(thread.isThread, isTrue);
      expect(thread.channelName, 'general');
      expect(thread.thread!.rootId, 'root-1');
      expect(results.dms, isEmpty);
    });

    test('sorts groups and threads by match, then recent activity', () {
      final results = _build(
        'plan',
        rows: [
          _room('plan', 10),
          _room('planning', 50),
          _room('old-plans', 5),
          _thread('t-word', 'Q4 plan review', 40),
          _thread('t-word-new', 'next plan', 60),
          _thread('t-middle', 'replanned', 99),
        ],
        channels: [
          _channel('plan', 'plan'),
          _channel('planning', 'planning'),
          _channel('old-plans', 'old-plans'),
          _channel('open-plan', 'plan-public', isMember: false),
        ],
      );

      expect(results.groups.map((r) => r.key), [
        'plan', // exact
        'planning', // starts with
        't-word-new', // word start, newer
        't-word', // word start
        'old-plans', // word start, oldest
        't-middle', // middle
        'open-plan', // not joined: last despite starting with
      ]);
    });

    test('puts DMs by their people and DM threads in DMs', () {
      final results = _build(
        'maya',
        rows: [
          _room('dm-1', 30, type: 'dm'),
          _room('dm-2', 80, type: 'dm'),
          _thread('dm-thread', 'Maya trip', 70, channelId: 'dm-2', type: 'dm'),
          _thread('group-thread', 'Ask maya', 90),
        ],
        channels: [
          _channel('general', 'general'),
          _channel('dm-1', 'dm', type: 'dm'),
          _channel('dm-2', 'dm', type: 'dm'),
        ],
        dmLabels: const {'dm-1': 'Maya', 'dm-2': 'Maya Kim, Sam'},
      );

      expect(results.groups.map((r) => r.key), ['group-thread']);
      expect(results.dms.map((r) => r.key), ['dm-1', 'dm-2', 'dm-thread']);
      expect(results.dms.last.channelName, 'Maya Kim, Sam');
    });

    test('ranks a description-only match after name matches', () {
      final results = _build(
        'design',
        channels: [
          _channel('a', 'general', description: 'Design talk'),
          _channel('b', 'ui-design'),
        ],
      );

      expect(results.groups.map((r) => r.key), ['b', 'a']);
    });
  });

  group('message hit threads', () {
    SearchHit hit(List<List<String>> tags, {int kind = 9}) => SearchHit(
      eventId: 'hit',
      content: 'hello',
      kind: kind,
      pubkey: 'author',
      createdAt: 1,
      score: 0,
      tags: tags,
    );

    test('a reply is in its thread; a broadcast reply is not', () {
      final reply = hit([
        ['e', 'root-1', '', 'root'],
        ['e', 'parent-1', '', 'reply'],
      ]);
      expect(searchHitThread(reply), (rootId: 'root-1', parentId: 'parent-1'));
      expect(
        searchHitThread(
          hit([
            ['e', 'root-1', '', 'reply'],
            ['broadcast', '1'],
          ]),
        ),
        isNull,
      );
      expect(searchHitThread(hit(const [])), isNull);
      expect(
        searchHitThread(
          hit([
            ['e', 'post', '', 'reply'],
          ], kind: 45003),
        ),
        isNull,
      );
    });

    test('the thread title comes from the Chats list', () {
      final rows = [_thread('root-1', '채널 검색 및 초대', 1)];
      expect(
        searchHitThreadTitle((rootId: 'root-1', parentId: 'root-1'), rows),
        '채널 검색 및 초대',
      );
      expect(
        searchHitThreadTitle((rootId: 'unknown', parentId: 'unknown'), rows),
        'Thread',
      );
    });
  });
}
