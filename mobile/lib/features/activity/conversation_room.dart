import '../../shared/identity_names/identity_names.dart';
import '../channels/channel.dart';
import '../channels/dm_channel_labels.dart';
import 'inbox_item.dart';

/// The chat room an inbox row stands for in the Channels + Threads view.
///
/// The row renders only the room's identity (avatar + name); each room kind
/// decides where that identity comes from.
sealed class ConversationRoom {
  const ConversationRoom();
}

/// A DM is shown as the other participant: their profile is the room's
/// picture and their name is the room's name, whoever sent last.
final class DmConversationRoom extends ConversationRoom {
  final String title;

  /// The other participant (first one for group DMs), or the sender when the
  /// participant list has no one else.
  final String personPubkey;

  const DmConversationRoom({required this.title, required this.personPubkey});
}

/// A channel's main timeline: the channel's letter avatar and `#name`.
final class ChannelConversationRoom extends ConversationRoom {
  final String channelId;
  final String channelName;

  const ChannelConversationRoom({
    required this.channelId,
    required this.channelName,
  });
}

/// A thread: its channel's letter avatar with a thread badge and
/// `#channel › thread name`.
final class ThreadConversationRoom extends ConversationRoom {
  final String channelId;
  final String channelName;
  final String threadRootId;

  const ThreadConversationRoom({
    required this.channelId,
    required this.channelName,
    required this.threadRootId,
  });
}

ConversationRoom resolveConversationRoom(
  InboxItem item, {
  required Channel? channel,
  required String? currentPubkey,
  required String senderLabel,
  IdentityNameSources? names,
}) {
  final senderPubkey = item.item.pubkey.toLowerCase();
  if (channel?.isDm ?? false) {
    final normalizedCurrent = currentPubkey?.toLowerCase();
    final counterpart = channel!.participantPubkeys
        .map((pk) => pk.toLowerCase())
        .where((pk) => pk != normalizedCurrent)
        .firstOrNull;
    return DmConversationRoom(
      title: resolveDmChannelDisplayLabel(
        channel,
        currentPubkey: currentPubkey,
        names: names,
      ),
      personPubkey: counterpart ?? senderPubkey,
    );
  }

  final channelId = channel?.id ?? item.item.channelId ?? '';
  final loadedName = channel?.name.trim() ?? '';
  final feedName = item.item.channelName.trim();
  final channelName = loadedName.isNotEmpty
      ? loadedName
      : feedName.isNotEmpty
      ? feedName
      : senderLabel;
  final threadRootId = item.threadRootId;
  return threadRootId == null
      ? ChannelConversationRoom(channelId: channelId, channelName: channelName)
      : ThreadConversationRoom(
          channelId: channelId,
          channelName: channelName,
          threadRootId: threadRootId,
        );
}

/// Room name; thread rooms need the resolved thread name from the caller.
String conversationRoomTitle(ConversationRoom room, {String? threadName}) =>
    switch (room) {
      DmConversationRoom(:final title) => title,
      ChannelConversationRoom(:final channelName) => '#$channelName',
      ThreadConversationRoom(:final channelName) =>
        '#$channelName › ${threadName ?? 'Thread'}',
    };
