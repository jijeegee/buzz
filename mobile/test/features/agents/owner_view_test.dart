import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/features/agents/owner_view/owner_view.dart';
import 'package:kuzz/features/channels/channel.dart';
import 'package:kuzz/features/channels/dm_channel_labels.dart';
import 'package:kuzz/features/chats/chat_list.dart';
import 'package:kuzz/shared/identity_names/identity_names.dart';
import 'package:kuzz/shared/profile/user_profile.dart';

const _dm = '11111111-1111-4111-8111-111111111111';
const _group = '22222222-2222-4222-8222-222222222222';
final _me = 'a' * 64;
final _agent = 'b' * 64;
final _bob = 'c' * 64;
final _carol = 'd' * 64;

ChatListRow _row(
  String channelId, {
  String? key,
  bool thread = false,
  String type = 'dm',
  List<String> members = const [],
  bool groupChat = false,
  int? memberCount,
  bool archived = false,
  int unread = 0,
  bool unreadMark = false,
}) => ChatListRow(
  isThread: thread,
  key: key ?? channelId,
  at: 100,
  atUs: 100000000,
  channelId: channelId,
  channelType: type,
  channelName: type == 'dm' ? 'DM' : 'Team',
  archived: archived,
  dmMembers: members,
  groupChat: groupChat,
  memberCount: memberCount,
  rootId: thread ? key : null,
  unread: unread,
  unreadMark: unreadMark,
);

Channel _personChat(List<String> participants) => Channel(
  id: _group,
  name: 'Team',
  channelType: 'stream',
  visibility: 'private',
  description: '',
  createdBy: '',
  createdAt: DateTime.utc(2026),
  memberCount: participants.length,
  participantPubkeys: participants,
  groupChat: true,
);

void main() {
  group('buildChatListFilter', () {
    test('asks for an agent\'s chats in lowercase', () {
      final filter = buildChatListFilter(
        limit: 50,
        asAgent: _agent.toUpperCase(),
      );
      expect(filter['as_agent'], _agent);
      expect(buildChatListFilter(limit: 50).containsKey('as_agent'), isFalse);
    });
  });

  group('agentChatChannels', () {
    test('a DM thread row first still names the other person', () {
      final channels = agentChatChannels([
        _row(_dm, key: 'e' * 64, thread: true),
        _row(_dm, members: [_bob, _agent]),
      ], _agent);

      final dm = channels[_dm]!;
      expect(dm.isDm, isTrue);
      expect(dm.isMember, isFalse);
      expect(dm.participantPubkeys, [_agent, _bob]);
      final names = IdentityNameSources(
        profiles: {_bob: UserProfile(pubkey: _bob, displayName: 'Bob')},
      );
      expect(
        resolveDmChannelDisplayLabel(dm, currentPubkey: _agent, names: names),
        'Bob',
      );
    });

    test('keeps the room row over a later empty thread row', () {
      final channels = agentChatChannels([
        _row(_dm, members: [_agent, _bob]),
        _row(_dm, key: 'e' * 64, thread: true),
      ], _agent);
      expect(channels[_dm]!.participantPubkeys, [_agent, _bob]);
    });

    test('group chat rows become private group chats', () {
      final channel = channelFromAgentChatRow(
        _row(
          _group,
          type: 'stream',
          members: [_bob, _carol],
          groupChat: true,
          memberCount: 4,
        ),
        _agent,
      );
      expect(channel.isGroupChat, isTrue);
      expect(channel.isPrivate, isTrue);
      expect(channel.memberCount, 4);
      expect(channel.participantPubkeys, [_agent, _bob, _carol]);
    });
  });

  group('loadAgentChatList', () {
    test('pages, drops archived and repeats, strips read state', () async {
      const cursor = ChatListCursor(atUs: 1, key: 'k');
      final calls = <ChatListCursor?>[];
      final rows = await loadAgentChatList((before) async {
        calls.add(before);
        return before == null
            ? ChatListPage(
                rows: [
                  _row(_dm, members: [_agent, _bob], unread: 3),
                  _row(_group, type: 'stream', archived: true),
                ],
                next: cursor,
              )
            : ChatListPage(
                rows: [
                  _row(_dm, members: [_agent, _bob]),
                  _row('x', unreadMark: true),
                ],
                next: null,
              );
      });
      expect(calls, [null, cursor]);
      expect(rows.map((row) => row.key), [_dm, 'x']);
      expect(rows.every((row) => row.unread == 0 && !row.unreadMark), isTrue);
    });

    test('stops at maxRows', () async {
      var pages = 0;
      final rows = await loadAgentChatList((before) async {
        pages++;
        return ChatListPage(
          rows: [_row('p$pages')],
          next: const ChatListCursor(atUs: 1, key: 'k'),
        );
      }, maxRows: 2);
      expect(rows, hasLength(2));
      expect(pages, 2);
    });
  });

  group('isMyAgent', () {
    test('trusts the profile owner tag', () {
      expect(
        isMyAgent(
          agentPubkey: _agent,
          me: _me,
          agentOwners: const {},
          profiles: {_agent: UserProfile(pubkey: _agent, ownerPubkey: _me)},
        ),
        isTrue,
      );
    });

    test('falls back to the agent directory', () {
      expect(
        isMyAgent(
          agentPubkey: _agent,
          me: _me,
          agentOwners: {_agent: _me},
          profiles: const {},
        ),
        isTrue,
      );
    });

    test('someone else\'s agent or a person is not mine', () {
      expect(
        isMyAgent(
          agentPubkey: _agent,
          me: _me,
          agentOwners: {_agent: _bob},
          profiles: const {},
        ),
        isFalse,
      );
      expect(
        isMyAgent(
          agentPubkey: _bob,
          me: _me,
          agentOwners: const {},
          profiles: const {},
        ),
        isFalse,
      );
      expect(
        isMyAgent(
          agentPubkey: _agent,
          me: null,
          agentOwners: {_agent: _me},
          profiles: const {},
        ),
        isFalse,
      );
    });
  });

  group('ownerReaders', () {
    test('lists owners outside the chat, not me or members', () {
      final readers = ownerReaders(
        channel: _personChat([_bob, _agent, _carol]),
        memberPubkeys: [_bob, _agent, _carol],
        me: _bob,
        agentOwners: {_agent: _me, _carol: _bob},
        profiles: const {},
      );
      expect(readers, [(agentPubkey: _agent, ownerPubkey: _me)]);
    });

    test('an owner in the chat reads nothing extra', () {
      final readers = ownerReaders(
        channel: _personChat([_me, _agent]),
        memberPubkeys: [_me, _agent, _bob],
        me: _bob,
        agentOwners: {_agent: _me},
        profiles: const {},
      );
      expect(readers, isEmpty);
    });
  });
}
