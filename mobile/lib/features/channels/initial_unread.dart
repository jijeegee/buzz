import '../../shared/read_state/read_state_format.dart';
import '../../shared/read_state/read_state_provider.dart';
import 'unread_badge/observed_unread_event.dart';

/// The observed events of [channelId] that [readState] still counts unread,
/// oldest first: top-level messages, or the replies of the thread [rootId].
List<ObservedUnreadEvent> observedUnreadEvents(
  ReadStateState readState,
  Map<String, Map<String, ObservedUnreadEvent>> observedByChannel,
  String channelId, {
  String? rootId,
}) {
  final channelReadAt = readState.effectiveTimestamp(channelId);
  return [
    for (final event
        in observedByChannel[channelId]?.values ??
            const <ObservedUnreadEvent>[])
      if (event.rootId == rootId &&
          event.createdAt >
              (observedUnreadEventReadAt(
                    event,
                    channelReadAt,
                    (rootId) =>
                        readState.effectiveTimestamp(threadContextKey(rootId)),
                    (messageId) =>
                        readState.effectiveTimestamp(msgContextKey(messageId)),
                  ) ??
                  0))
        event,
  ]..sort((a, b) => a.createdAt.compareTo(b.createdAt));
}
