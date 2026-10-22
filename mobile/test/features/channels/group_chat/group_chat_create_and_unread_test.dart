import 'package:flutter_test/flutter_test.dart';
import 'package:kuzz/features/channels/channel.dart';
import 'package:kuzz/features/channels/channel_management_provider.dart';
import 'package:kuzz/features/channels/unread_badge/observed_unread_event.dart';
import 'package:kuzz/shared/community/community.dart';
import 'package:kuzz/shared/push/push_subscription_provider.dart';
import 'package:nostr/nostr.dart' as nostr;

final _me = 'a' * 64;
final _ann = 'b' * 64;
final _bob = 'c' * 64;

const _chatId = '123e4567-e89b-42d3-a456-426614174000';
const _groupId = '123e4567-e89b-42d3-a456-426614174001';
const _dmId = '123e4567-e89b-42d3-a456-426614174002';

Channel _channel(String id, {String type = 'stream', bool chat = false}) =>
    Channel(
      id: id,
      name: '',
      channelType: type,
      visibility: 'private',
      description: '',
      createdBy: '',
      createdAt: DateTime(2026),
      memberCount: 3,
      isMember: true,
      groupChat: chat,
    );

void main() {
  group('create a chat', () {
    test('one kind 9007: flag, private stream, name, people (not me)', () {
      final tags = buildCreateGroupChatTags(
        channelId: _chatId,
        pubkeys: [_ann, _bob.toUpperCase(), _ann, _me],
        name: '  Trip ',
        currentPubkey: _me,
      );
      expect(tags, [
        ['h', _chatId],
        ['group_chat'],
        ['visibility', 'private'],
        ['channel_type', 'stream'],
        ['name', 'Trip'],
        ['p', _ann],
        ['p', _bob],
      ]);
    });

    test('no name tag when unnamed', () {
      final tags = buildCreateGroupChatTags(
        channelId: _chatId,
        pubkeys: [_ann, _bob],
        name: '   ',
      );
      expect(tags.where((tag) => tag.first == 'name'), isEmpty);
    });

    test('at most 100 people at creation', () {
      final people = [
        for (var i = 0; i < groupChatCreateLimit + 1; i++)
          i.toRadixString(16).padLeft(64, '0'),
      ];
      expect(
        () => buildCreateGroupChatTags(channelId: _chatId, pubkeys: people),
        throwsArgumentError,
      );
      expect(
        buildCreateGroupChatTags(
          channelId: _chatId,
          pubkeys: people.take(groupChatCreateLimit),
        ).where((tag) => tag.first == 'p'),
        hasLength(groupChatCreateLimit),
      );
    });

    test('invite-sent:N in the OK reply', () {
      expect(inviteSentCountFromReply('invite-sent:2'), 2);
      expect(inviteSentCountFromReply('invite-sent: 3 rate-limited'), 3);
      expect(inviteSentCountFromReply(''), isNull);
    });

    test('the note after creating', () {
      CreateGroupChatResult result(
        int invited,
        int picked, {
        bool limited = false,
      }) => CreateGroupChatResult(
        channel: _channel(_chatId, chat: true),
        invitedCount: invited,
        pickedCount: picked,
        rateLimited: limited,
      );
      expect(result(0, 3).notice, isNull);
      expect(result(1, 3).notice, 'Invitation sent to 1');
      expect(result(3, 3).notice, 'Invitations sent');
      expect(result(0, 3, limited: true).notice, tooManyInvitationsMessage);
    });
  });

  group('unread and push like a DM', () {
    test('every chat message counts toward the badges', () {
      final event = makeObservedUnreadEvent(
        id: 'e1',
        createdAt: 1,
        rootId: null,
        highPriority: false,
        channelType: 'stream',
        isThreadedReply: false,
        isPersonChat: true,
      );
      expect(event.countsTowardBadge, isTrue);
      expect(event.countsTowardAppBadge, isTrue);
      final groupEvent = makeObservedUnreadEvent(
        id: 'e2',
        createdAt: 1,
        rootId: null,
        highPriority: false,
        channelType: 'stream',
        isThreadedReply: false,
      );
      expect(groupEvent.countsTowardBadge, isFalse);
    });

    test('chats subscribe channel-wide; muted ones are ignored', () {
      final subscriptions = desiredKuzzPushSubscriptions(
        community: Community.create(
          name: 'Team',
          relayUrl: 'https://relay.example.com',
          nsec: nostr.Keys.generate().nsec,
        ),
        channels: [
          _channel(_chatId, chat: true),
          _channel(_groupId),
          _channel(_dmId, type: 'dm'),
        ],
        mutedChannelIds: const [_dmId],
      )!;
      final channelWide = {
        for (final subscription in subscriptions) ...?subscription.filter.hTags,
      };
      expect(channelWide, contains(_chatId));
      expect(channelWide, isNot(contains(_groupId)));
      expect(channelWide, isNot(contains(_dmId)));
      final ignored = {
        for (final subscription in subscriptions)
          for (final filter in subscription.ignore) ...?filter.hTags,
      };
      expect(ignored, {_dmId});
    });
  });
}
