import '../channels/channel.dart';
import 'chat_list.dart';

/// The room a Chats row belongs to, built from the row, to open at once when
/// the channel list does not hold it yet (nothing saved, or a room newer than
/// the saved list). The room page draws its stored rows and swaps in the
/// listed room and its details as they arrive; a room the relay says is gone
/// keeps its usual not-found handling.
Channel channelFromChatRow(ChatListRow row) {
  final latestAt = row.latest?.createdAt ?? (row.at > 0 ? row.at : null);
  DateTime? at(int? seconds) => seconds == null
      ? null
      : DateTime.fromMillisecondsSinceEpoch(seconds * 1000, isUtc: true);
  final people = [for (final pubkey in row.dmMembers) pubkey.toLowerCase()];
  return Channel(
    id: row.channelId,
    name: row.channelName,
    channelType: row.channelType,
    // Unknown until the details load; private never offers more than a
    // member may do.
    visibility: 'private',
    description: '',
    createdBy: '',
    createdAt: DateTime.fromMillisecondsSinceEpoch(0, isUtc: true),
    memberCount: row.memberCount ?? people.length,
    lastMessageAt: at(latestAt),
    archivedAt: row.archived ? at(latestAt ?? 0) : null,
    participantPubkeys: people,
    // Chats lists the account's own rooms.
    isMember: true,
    groupChat: row.groupChat,
  );
}

/// The room [channelId] from the Chats [rows] that name it (its room row
/// first, else the one with the most people), or `null` when none does.
Channel? channelFromChatRows(Iterable<ChatListRow> rows, String channelId) {
  ChatListRow? best;
  for (final row in rows) {
    if (row.channelId != channelId) continue;
    if (best == null ||
        (best.isThread && !row.isThread) ||
        (best.isThread == row.isThread &&
            row.dmMembers.length > best.dmMembers.length)) {
      best = row;
    }
  }
  return best == null ? null : channelFromChatRow(best);
}

/// A room known only by its id (a link or notification no row names): the
/// room page loads its name and details.
Channel channelFromId(String channelId) => Channel(
  id: channelId,
  name: '',
  channelType: 'stream',
  visibility: 'private',
  description: '',
  createdBy: '',
  createdAt: DateTime.fromMillisecondsSinceEpoch(0, isUtc: true),
  memberCount: 0,
  isMember: true,
);
