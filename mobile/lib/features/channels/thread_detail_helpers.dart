part of 'thread_detail_page.dart';

Widget _trackActiveThreadScrollPosition(
  Widget child,
  ObjectRef<ScrollPosition?> activePosition,
) => Builder(
  builder: (context) {
    activePosition.value = Scrollable.of(context).position;
    return child;
  },
);

/// Whether [message] sorts after [other] in the thread's timeline order.
bool _isNewerThan(TimelineMessage message, TimelineMessage other) {
  final order = message.createdAt.compareTo(other.createdAt);
  return order != 0 ? order > 0 : message.id.compareTo(other.id) > 0;
}

bool _isDeletedBy(Iterable<NostrEvent> events, String messageId) {
  for (final event in events) {
    if (event.kind != EventKind.deletion &&
        event.kind != EventKind.nip29DeleteEvent) {
      continue;
    }
    if (event.tags.any(
      (tag) => tag.length >= 2 && tag[0] == 'e' && tag[1] == messageId,
    )) {
      return true;
    }
  }
  return false;
}

/// Build a lightweight summary for a nested thread (reply that has its own
/// replies). Same logic as the top-level [ThreadSummary] but kept local to
/// avoid coupling.
ThreadSummary _buildNestedSummary(
  String messageId,
  List<TimelineMessage> children,
) {
  final seen = <String>{};
  final participants = <String>[];
  for (var i = children.length - 1; i >= 0 && participants.length < 3; i--) {
    final pk = children[i].pubkey.toLowerCase();
    if (seen.add(pk)) participants.add(pk);
  }
  return ThreadSummary(
    threadHeadId: messageId,
    replyCount: children.length,
    participantPubkeys: participants.reversed.toList(),
    lastReplyAt: children.last.createdAt,
  );
}

/// Thread-scoped typing status with optional size animation.
class _ThreadTypingIndicator extends StatelessWidget {
  final String channelId;
  final List<TypingEntry> entries;
  final bool animated;

  const _ThreadTypingIndicator({
    required this.channelId,
    required this.entries,
    this.animated = true,
  });

  @override
  Widget build(BuildContext context) {
    final child = entries.isEmpty
        ? const SizedBox.shrink()
        : ChannelTypingIndicator(channelId: channelId, entries: entries);
    if (!animated || MediaQuery.disableAnimationsOf(context)) return child;
    return AnimatedSize(
      duration: const Duration(milliseconds: 180),
      curve: Curves.easeOutCubic,
      alignment: Alignment.bottomCenter,
      child: child,
    );
  }
}

/// Display evidence while the authoritative thread query has no value (still
/// loading, or failed on first load): the route snapshot plus live, cached
/// and optimistic replies. The page keeps the query's loading/error status,
/// so this list is never presented as complete, and nothing is re-queried.
List<TimelineMessage> _provisionalThreadMessages(
  List<TimelineMessage> routeSnapshot,
  List<TimelineMessage> observed,
  List<NostrEvent> liveChannelEvents,
) {
  final byId = <String, TimelineMessage>{
    for (final message in routeSnapshot)
      if (!_isDeletedBy(liveChannelEvents, message.id)) message.id: message,
    for (final message in observed) message.id: message,
  };
  return byId.values.toList()..sort((a, b) {
    final order = a.createdAt.compareTo(b.createdAt);
    return order != 0 ? order : a.id.compareTo(b.id);
  });
}
