import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/relay/relay.dart';
import 'timeline_message.dart';

/// A quoted message fetched by id, with the channel it was posted in.
typedef QuotedMessage = ({String? channelId, TimelineMessage message});

/// Looks up a quoted event by id when it is not already loaded on screen.
///
/// Resolves to null when the relay has no such content event (deleted,
/// inaccessible, or never existed), so the header can show its fallback.
final quotedMessageProvider = FutureProvider.autoDispose
    .family<QuotedMessage?, String>((ref, eventId) async {
      final events = await ref
          .read(relaySessionProvider.notifier)
          .fetchHistory(
            NostrFilter(
              kinds: EventKind.channelTimelineContentKinds,
              ids: [eventId],
              limit: 1,
            ),
          );
      for (final event in events) {
        if (event.id.toLowerCase() != eventId) continue;
        final formatted = formatTimeline([event]);
        if (formatted.isEmpty) return null;
        return (channelId: event.channelId, message: formatted.first);
      }
      return null;
    });
