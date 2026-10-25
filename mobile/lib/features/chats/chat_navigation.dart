import '../channels/channel.dart';
import 'chat_list.dart';

/// Opens a visible, account-scoped Chats row before the full room directory
/// arrives. The relay's chat_list contains only the reader's active memberships.
/// Details and the live directory replace this snapshot after navigation.
///
/// Unknown ownership and visibility stay unknown: a list row must not grant
/// management actions. Sending is still authorized by the relay, as when a
/// previously loaded room is opened while reconnecting.
Channel channelFromChatRow(ChatListRow row, String currentPubkey) => Channel(
  id: row.channelId,
  name: row.channelName,
  channelType: row.channelType,
  visibility: '',
  description: '',
  createdBy: '',
  createdAt: DateTime.fromMillisecondsSinceEpoch(0, isUtc: true),
  memberCount: row.memberCount ?? (row.isDm ? row.dmMembers.length + 1 : 0),
  lastMessageAt: DateTime.fromMillisecondsSinceEpoch(
    row.at * 1000,
    isUtc: true,
  ),
  archivedAt: row.archived
      ? DateTime.fromMillisecondsSinceEpoch(0, isUtc: true)
      : null,
  participantPubkeys: row.isPersonChat
      ? {currentPubkey, ...row.dmMembers}.toList()
      : const [],
  isMember: true,
  groupChat: row.groupChat,
  goalsEnabled: false,
);
