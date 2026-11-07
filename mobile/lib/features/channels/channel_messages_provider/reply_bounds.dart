part of '../channel_messages_provider.dart';

/// Bounding the replies a room keeps in memory.
extension _ReplyBounds on ChannelMessagesNotifier {
  List<NostrEvent> _boundedReplies(Iterable<NostrEvent> events) {
    final sorted = events.toList()
      ..sort((a, b) => compareThreadRepliesChronologically(b, a));
    final counts = <String?, int>{};
    final seen = <String>{};
    final retained = <NostrEvent>[];
    for (final event in sorted) {
      if (!seen.add(event.id)) continue;
      final root = event.threadReference.rootId;
      final count = counts[root] ?? 0;
      if (count >= _maxCachedRepliesPerRoot) continue;
      retained.add(event);
      counts[root] = count + 1;
      if (retained.length == _maxCachedReplies) break;
    }
    return retained;
  }

  List<NostrEvent> _boundEventReplies(List<NostrEvent> events) {
    final replies = events.where(
      (event) =>
          EventKind.channelTimelineContentKinds.contains(event.kind) &&
          event.threadReference.parentId != null,
    );
    final retained = _boundedReplies(replies).map((event) => event.id).toSet();
    _reconcileEvictedReplies(replies, retained);
    return events
        .where(
          (event) =>
              !EventKind.channelTimelineContentKinds.contains(event.kind) ||
              event.threadReference.parentId == null ||
              retained.contains(event.id),
        )
        .toList();
  }

  void _trimReplyOverlay() {
    final replies = _windowStore.liveOverlay
        .where((event) => event.threadReference.parentId != null)
        .toList();
    if (replies.length <= _maxCachedRepliesPerRoot) return;
    final retained = _boundedReplies(replies).map((event) => event.id).toSet();
    if (retained.length == replies.length) return;
    _reconcileEvictedReplies(replies, retained);
    _windowStore = _windowStore.copyWith(
      liveOverlay: _windowStore.liveOverlay
          .where(
            (event) =>
                event.threadReference.parentId == null ||
                retained.contains(event.id),
          )
          .toList(),
    );
  }
}
