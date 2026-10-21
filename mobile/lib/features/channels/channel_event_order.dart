import '../../shared/relay/relay.dart';

/// Render order for top-level channel timelines.
///
/// The relay pages newest timestamp first and ascending id within a second,
/// and channel timelines reverse that composite order for display, matching
/// desktop: oldest timestamp first and descending id within a second.
int compareChannelTimelineEventsChronologically(
  NostrEvent left,
  NostrEvent right,
) {
  final createdAt = left.createdAt.compareTo(right.createdAt);
  return createdAt != 0 ? createdAt : right.id.compareTo(left.id);
}

/// Render order for thread replies.
///
/// Desktop threads use ascending id within a second, independently of the
/// channel-window display order.
int compareThreadRepliesChronologically(NostrEvent left, NostrEvent right) {
  final createdAt = left.createdAt.compareTo(right.createdAt);
  return createdAt != 0 ? createdAt : left.id.compareTo(right.id);
}

/// Sorts [events] for a channel timeline and returns them: in the relay's
/// acceptance order when every event carries an order number (`kuzz-seq`),
/// else by [compareChannelTimelineEventsChronologically]. Mixing the two
/// orders pair by pair would not be a consistent sort.
List<NostrEvent> sortChannelTimeline(List<NostrEvent> events) => events
  ..sort(
    _allSequenced(events)
        ? _compareSequence
        : compareChannelTimelineEventsChronologically,
  );

/// Sorts thread [events] like [sortChannelTimeline], falling back to
/// [compareThreadRepliesChronologically]; [newestFirst] reverses the order.
List<NostrEvent> sortThreadReplies(
  List<NostrEvent> events, {
  bool newestFirst = false,
}) {
  final compare = _allSequenced(events)
      ? _compareSequence
      : compareThreadRepliesChronologically;
  return events..sort(newestFirst ? (a, b) => compare(b, a) : compare);
}

bool _allSequenced(List<NostrEvent> events) =>
    events.every((event) => event.channelSequence != null);

int _compareSequence(NostrEvent left, NostrEvent right) =>
    left.channelSequence!.compareTo(right.channelSequence!);
