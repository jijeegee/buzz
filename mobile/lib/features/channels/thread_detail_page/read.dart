part of '../thread_detail_page.dart';

/// Marks an open thread's replies read, and the thread as a whole for the
/// Chats list and other devices. The owner view reads someone else's chat,
/// so it passes `enabled: false`. A reply from someone else that arrives
/// dated at or before the read sends the read again ([useLateArrivals]).
void _useThreadRead(
  WidgetRef ref, {
  required bool enabled,
  required String channelId,
  required String rootId,
  required List<TimelineMessage> replies,
  required ObjectRef<List<ObservedUnreadEvent>?> openedUnread,
  required String? currentPubkey,
}) {
  final visibleReplyReadKey = replies
      .map((reply) => '${reply.id}:${reply.createdAt}')
      .join(',');
  final readAt = replies.isEmpty
      ? null
      : replies.map((reply) => reply.createdAt).reduce(math.max);
  final lateArrivalCount = useLateArrivals(
    rootId,
    [
      for (final reply in replies)
        (id: reply.id, pubkey: reply.pubkey, createdAt: reply.createdAt),
    ],
    readAt,
    currentPubkey,
    ref.read(channelMessagesProvider(channelId).notifier).arrivedLive,
  );

  useEffect(() {
    if (!enabled || readAt == null) return null;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      final readState = ref.read(readStateProvider.notifier);
      for (final reply in replies) {
        readState.markContextRead(msgContextKey(reply.id), reply.createdAt);
      }
      // Opening a thread at its first unread reply still reads all of them.
      for (final event in openedUnread.value ?? const <ObservedUnreadEvent>[]) {
        readState.markContextRead(msgContextKey(event.id), event.createdAt);
      }
      readState.markContextRead(
        threadContextKey(rootId),
        readAt,
        resend: lateArrivalCount > 0,
      );
    });
    return null;
  }, [rootId, enabled, visibleReplyReadKey]);
}
