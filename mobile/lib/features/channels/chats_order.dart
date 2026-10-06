import 'channel.dart';

/// Personal pins first, then actual message time; IDs stabilize ties/empty chats.
List<Channel> sortChats(Iterable<Channel> channels, Set<String> pinnedIds) =>
    channels.toList()..sort((a, b) {
      final pin = (pinnedIds.contains(b.id) ? 1 : 0).compareTo(
        pinnedIds.contains(a.id) ? 1 : 0,
      );
      if (pin != 0) return pin;
      final aTime = a.lastMessageAt;
      final bTime = b.lastMessageAt;
      final recent = aTime == null
          ? (bTime == null ? 0 : 1)
          : bTime == null
          ? -1
          : bTime.compareTo(aTime);
      return recent != 0 ? recent : a.id.compareTo(b.id);
    });
