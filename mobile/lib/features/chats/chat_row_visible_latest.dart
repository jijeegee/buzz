import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/relay/relay.dart';
import '../../shared/message_db/message_store.dart';
import '../channels/channel_room_store.dart';
import '../people/blocked_users_provider.dart';
import 'chat_list.dart';

/// Which row a [chatRowVisibleLatestProvider] answers for: its channel, its
/// thread root (null for a room) and the time of its latest message.
typedef ChatRowVisibleLatestKey = ({
  String channelId,
  String? rootId,
  int before,
});

/// For a Chats row whose latest message is from someone I blocked: the
/// newest earlier message there from someone I did not block, as stored on
/// this device (the message database). Null when none is stored; the row
/// then shows a neutral line instead of the blocked message.
final chatRowVisibleLatestProvider = FutureProvider.autoDispose
    .family<NostrEvent?, ChatRowVisibleLatestKey>((ref, key) async {
      final scope = ref.watch(messageScopeProvider);
      if (scope == null) return null;
      final blocked = {
        for (final id in ref.watch(blockedPubkeysProvider)) id.toLowerCase(),
      };
      final events = ref
          .read(channelRoomStoreProvider)
          .readRowsBefore(
            scope,
            key.channelId,
            before: key.before,
            rootId: key.rootId,
          );
      return latestVisibleChatMessage(
        events,
        rootId: key.rootId,
        before: key.before,
        isBlocked: blocked.contains,
      );
    }, retry: (_, _) => null);
