import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/features/channels/channel.dart';
import 'package:kuzz/features/channels/channel_management_provider.dart';
import 'package:kuzz/features/channels/compose_bar.dart';
import 'package:kuzz/features/chats/chat_list.dart';
import 'package:kuzz/features/groups/groups_api.dart';
import 'package:kuzz/shared/relay/relay.dart';

const _chatId = '11111111-1111-4111-8111-111111111111';
final _ann = 'a' * 64;
final _bob = 'b' * 64;

NostrEvent _meta(List<List<String>> tags) => NostrEvent(
  id: 'e' * 64,
  pubkey: 'f' * 64,
  createdAt: 1,
  kind: 39000,
  tags: [
    ['d', _chatId],
    ...tags,
  ],
  content: '',
  sig: '',
);

Channel _channel({
  required String id,
  String name = '',
  String type = 'stream',
  bool groupChat = false,
}) => Channel(
  id: id,
  name: name,
  channelType: type,
  visibility: 'private',
  description: '',
  createdBy: '',
  createdAt: DateTime(2026),
  memberCount: 3,
  isMember: true,
  groupChat: groupChat,
);

void main() {
  group('group chat flag', () {
    test('a 39000 group_chat tag marks the channel a chat', () {
      final data = ChannelData.fromEvent(
        _meta([
          ['t', 'stream'],
          ['private'],
          ['group_chat'],
        ]),
      );
      expect(data.groupChat, isTrue);
      final details = channelDetailsFromEvent(
        _meta([
          ['t', 'stream'],
          ['group_chat'],
        ]),
      );
      expect(details.groupChat, isTrue);
    });

    test('no tag, or a DM, is never a chat', () {
      expect(
        ChannelData.fromEvent(
          _meta([
            ['t', 'stream'],
          ]),
        ).groupChat,
        isFalse,
      );
      expect(
        ChannelData.fromEvent(
          _meta([
            ['t', 'dm'],
            ['group_chat'],
          ]),
        ).groupChat,
        isFalse,
      );
    });

    test('Channel JSON and getters', () {
      final chat = Channel.fromJson({
        'id': _chatId,
        'name': '',
        'channel_type': 'stream',
        'visibility': 'private',
        'created_by': '',
        'created_at': '2026-10-22T00:00:00Z',
        'member_count': 4,
        'group_chat': true,
      });
      expect(chat.isGroupChat, isTrue);
      expect(chat.isPersonChat, isTrue);
      final dm = _channel(id: 'dm', type: 'dm', groupChat: true);
      expect(dm.isGroupChat, isFalse);
      expect(dm.isPersonChat, isTrue);
      expect(_channel(id: 'g').isPersonChat, isFalse);
    });

    test('chat list rows carry group_chat, dm_members and member_count', () {
      final row = ChatListRow.fromJson({
        'type': 'room',
        'key': _chatId,
        'at': 10,
        'at_us': 10000000,
        'channel_id': _chatId,
        'channel_type': 'stream',
        'channel_name': '',
        'group_chat': true,
        'dm_members': [_ann, _bob],
        'member_count': 5,
      })!;
      expect(row.isGroupChat, isTrue);
      expect(row.isPersonChat, isTrue);
      expect(row.dmMembers, [_ann, _bob]);
      expect(row.memberCount, 5);
      expect(ChatListRow.fromJson(row.toJson())!.isGroupChat, isTrue);
    });

    test('invitations read the chat fields inside channel (camelCase)', () {
      final invitation = GroupInvitation.fromJson({
        'id': 'inv-1',
        'channel': {
          'id': _chatId,
          'name': '',
          'description': '',
          'channelType': 'stream',
          'visibility': 'private',
          'isMember': false,
          'groupChat': true,
          'memberPubkeys': [_ann, _bob],
          'memberCount': 6,
        },
        'inviter': _ann,
        'createdAt': '2026-10-22T00:00:00Z',
      });
      final group = invitation.group;
      expect(group.groupChat, isTrue);
      expect(group.memberPubkeys, [_ann, _bob]);
      expect(group.memberCount, 6);
      final channel = group.toChannel();
      expect(channel.isGroupChat, isTrue);
      expect(channel.participantPubkeys, [_ann, _bob]);
    });

    test('invitation snake_case fields are read too', () {
      final invitation = GroupInvitation.fromJson({
        'id': 'inv-1',
        'channel': {
          'id': _chatId,
          'name': 'Trip',
          'description': '',
          'channelType': 'stream',
          'isMember': false,
        },
        'group_chat': true,
        'member_pubkeys': [_ann],
        'member_count': 2,
        'inviter': _ann,
        'createdAt': '2026-10-22T00:00:00Z',
      });
      expect(invitation.group.groupChat, isTrue);
      expect(invitation.group.memberPubkeys, [_ann]);
      expect(invitation.group.memberCount, 2);
    });
  });

  test('#name suggestions never offer chats of people', () {
    final results = filterChannels([
      _channel(id: 'g1', name: 'general'),
      _channel(id: 'c1', name: 'gang', groupChat: true),
      _channel(id: 'd1', name: 'gary', type: 'dm'),
    ], 'g');
    expect(results.map((c) => c.id), ['g1']);
  });
}
