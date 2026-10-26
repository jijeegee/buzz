import 'package:flutter_hooks/flutter_hooks.dart';

/// What [lateArrivals] needs of a message.
typedef ArrivalEvent = ({String id, String pubkey, int createdAt});

/// Ids of [events] from others not yet in [seen] that [arrived] (reached
/// the page as new events, not through a page or by-id load), dated at or
/// before [readAt]. Adds every id to [seen], so each event is judged once.
List<String> lateArrivals(
  Iterable<ArrivalEvent> events,
  Set<String> seen,
  int readAt,
  String? selfPubkey,
  bool Function(String eventId) arrived,
) {
  final self = selfPubkey?.toLowerCase();
  final late = <String>[];
  for (final event in events) {
    if (!seen.add(event.id)) continue;
    if (event.pubkey.toLowerCase() != self &&
        event.createdAt <= readAt &&
        arrived(event.id)) {
      late.add(event.id);
    }
  }
  return late;
}

/// How many messages from others have [arrived] in [events] while
/// [contextId] is open, dated at or before [readAt]. Older pages loaded while
/// scrolling are history, not arrivals. The relay counts unread
/// by arrival, and a read time is turned into the newest arrival dated at or
/// before it when the read is stored; a message dated earlier (a sender clock
/// behind, a send signed before it went out) that arrives after leaves that
/// read unchanged, so nothing would clear its unread count. A page sends its
/// read again (`resend`) when this grows. The first build of a context only
/// records what is already there.
int useLateArrivals(
  String contextId,
  Iterable<ArrivalEvent> events,
  int? readAt,
  String? selfPubkey,
  bool Function(String eventId) arrived,
) {
  final seen = useRef<({String contextId, Set<String> ids})?>(null);
  final count = useRef(0);
  final current = seen.value;
  if (current == null || current.contextId != contextId) {
    seen.value = (
      contextId: contextId,
      ids: {for (final event in events) event.id},
    );
    count.value = 0;
  } else if (readAt == null) {
    current.ids.addAll(events.map((event) => event.id));
  } else {
    count.value += lateArrivals(
      events,
      current.ids,
      readAt,
      selfPubkey,
      arrived,
    ).length;
  }
  return count.value;
}
