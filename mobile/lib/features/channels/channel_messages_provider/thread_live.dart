part of '../channel_messages_provider.dart';

/// Keeping held threads current as their replies arrive live
/// (docs/message-sync.md#open-room-and-thread-requests). Desktop merges live
/// replies into the thread the same way.
extension _ThreadLive on ChannelMessagesNotifier {
  /// A reply reached the room. An authoritative one joins its thread's
  /// window when that window can place it ([_foldLiveReply]); otherwise the
  /// thread is marked stale: an open thread refetches at once, a closed,
  /// retained one when it is reopened.
  void _updateThreadReplies(NostrEvent event, {required bool authoritative}) {
    if (!EventKind.channelTimelineContentKinds.contains(event.kind)) return;
    final thread = event.threadReference;
    if (thread.parentId == null) return;

    final rootId = thread.rootId;
    if (rootId != null) {
      final args = ThreadRepliesArgs(channelId: channelId, rootId: rootId);
      if (!authoritative || !_foldLiveReply(args, event)) {
        _markThreadStale(args);
      }
    }
    // A thread keyed by the reply's parent rather than its root pages that
    // parent's relay window, which need not hold this reply: it refetches.
    final parentId = thread.parentId;
    if (parentId != null && parentId != rootId) {
      _markThreadStale(
        ThreadRepliesArgs(channelId: channelId, rootId: parentId),
      );
    }
  }

  /// Folds [reply] into the window of [args] without asking the relay.
  /// Refused while the thread's query is loading or its last refresh failed
  /// (its page may not hold the reply, or a gap may lie under it), while other sends in the thread await proof (only a scan settles
  /// them; the reply settles its own), or when the window cannot place it
  /// ([foldLiveThreadReply]).
  bool _foldLiveReply(ThreadRepliesArgs args, NostrEvent reply) {
    final window = _settledThreadWindow(args);
    if (window == null) return false;
    final pending = unconfirmedThreadReplyIds(args.rootId)..remove(reply.id);
    return pending.isEmpty && window.foldLive(reply);
  }
}
