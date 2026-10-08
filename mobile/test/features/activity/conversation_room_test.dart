import 'package:buzz/features/activity/conversation_room.dart';
import 'package:buzz/features/activity/feed_item.dart';
import 'package:buzz/features/activity/inbox_item.dart';
import 'package:buzz/features/channels/channel.dart';
import 'package:flutter_test/flutter_test.dart';

const me = 'aaaa000000000000000000000000000000000000000000000000000000000000';
const alice =
    'bbbb000000000000000000000000000000000000000000000000000000000000';

InboxItem inboxItem({
  String channelId = 'ch1',
  String pubkey = me,
  List<List<String>> tags = const [],
}) {
  final feedItem = FeedItem(
    id: 'ev1',
    kind: 9,
    pubkey: pubkey,
    content: 'hello',
    createdAt: 100,
    channelId: channelId,
    channelName: 'kuzz',
    tags: tags,
    category: 'activity',
  );
  return InboxItem(
    conversationId: 'ev1',
    id: 'ev1',
    item: feedItem,
    groupItems: [feedItem],
    categories: const ['activity'],
    isActionRequired: false,
    latestActivityAt: 100,
  );
}

Channel channel({
  required String id,
  required String name,
  String channelType = 'stream',
  List<String> participants = const [],
  List<String> participantPubkeys = const [],
}) => Channel(
  id: id,
  name: name,
  channelType: channelType,
  visibility: channelType == 'dm' ? 'private' : 'open',
  description: '',
  createdBy: 'x',
  createdAt: DateTime(2025),
  memberCount: 2,
  isMember: true,
  participants: participants,
  participantPubkeys: participantPubkeys,
);

void main() {
  test('a DM room is the other participant even when I sent last', () {
    final room = resolveConversationRoom(
      inboxItem(channelId: 'dm1'),
      channel: channel(
        id: 'dm1',
        name: 'Alice',
        channelType: 'dm',
        participants: const ['Me', 'Alice'],
        participantPubkeys: const [me, alice],
      ),
      currentPubkey: me,
      senderLabel: 'Me',
    );
    expect(room, isA<DmConversationRoom>());
    expect((room as DmConversationRoom).personPubkey, alice);
    expect(conversationRoomTitle(room), 'Alice');
  });

  test('a channel main message is the channel room', () {
    final room = resolveConversationRoom(
      inboxItem(),
      channel: channel(id: 'ch1', name: 'kuzz'),
      currentPubkey: me,
      senderLabel: 'Me',
    );
    expect(room, isA<ChannelConversationRoom>());
    expect(conversationRoomTitle(room), '#kuzz');
  });

  test('a thread reply is the thread room named after the thread', () {
    final room = resolveConversationRoom(
      inboxItem(
        tags: const [
          ['e', 'root1', '', 'root'],
          ['e', 'parent1', '', 'reply'],
        ],
      ),
      channel: channel(id: 'ch1', name: 'kuzz'),
      currentPubkey: me,
      senderLabel: 'Me',
    );
    expect(room, isA<ThreadConversationRoom>());
    expect((room as ThreadConversationRoom).threadRootId, 'root1');
    expect(
      conversationRoomTitle(room, threadName: 'Activity filter'),
      '#kuzz › Activity filter',
    );
    expect(conversationRoomTitle(room), '#kuzz › Thread');
  });

  test('an unloaded channel falls back to the feed channel name', () {
    final room = resolveConversationRoom(
      inboxItem(),
      channel: null,
      currentPubkey: me,
      senderLabel: 'Me',
    );
    expect(conversationRoomTitle(room), '#kuzz');
  });
}
