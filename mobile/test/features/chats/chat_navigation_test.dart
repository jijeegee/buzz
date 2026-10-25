import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/features/chats/chat_list.dart';
import 'package:kuzz/features/chats/chat_navigation.dart';

void main() {
  test('a saved DM can compose without inferring management permissions', () {
    final channel = channelFromChatRow(
      const ChatListRow(
        isThread: false,
        key: 'room',
        at: 10,
        atUs: 10000000,
        channelId: 'room',
        channelType: 'dm',
        channelName: 'Mina',
        dmMembers: ['them'],
      ),
      'me',
    );
    expect(channel.isMember, isTrue);
    expect(channel.isOneToOneDm, isTrue);
    expect(channel.displayLabel(currentPubkey: 'me'), 'Mina');
    expect(channel.createdBy, isEmpty);
    expect(channel.canAddMembers('admin'), isFalse);
    expect(channel.canJoin, isFalse);
    expect(channel.goalsEnabled, isFalse);
    expect(channel.historyAll, isFalse);
  });

  test('a saved archived group remains archived and has no inferred owner', () {
    final channel = channelFromChatRow(
      const ChatListRow(
        isThread: true,
        key: 'thread',
        at: 10,
        atUs: 10000000,
        channelId: 'room',
        channelType: 'stream',
        channelName: 'Group',
        archived: true,
        groupChat: true,
        memberCount: 20,
        dmMembers: ['a', 'b'],
      ),
      'me',
    );
    expect(channel.id, 'room');
    expect(channel.isArchived, isTrue);
    expect(channel.isGroupChat, isTrue);
    expect(channel.memberCount, 20);
    expect(channel.canAddMembers('admin'), isFalse);
    expect(channel.createdBy, isEmpty);
  });
}
