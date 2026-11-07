import '../../shared/relay/relay.dart';

/// Bounded identity-only evidence for replies whose payloads may be evicted.
class ThreadReplyOwnership {
  final Map<String, String> _roots = {};

  /// Remembers recent ownership without retaining event payloads.
  void record(Iterable<NostrEvent> events) {
    for (final event in events) {
      final thread = event.threadReference;
      if (!EventKind.channelTimelineContentKinds.contains(event.kind) ||
          thread.parentId == null ||
          thread.rootId == null) {
        continue;
      }
      _roots.remove(event.id);
      _roots[event.id] = thread.rootId!;
      while (_roots.length > 8192) {
        _roots.remove(_roots.keys.first);
      }
    }
  }

  /// Returns known ownership or null when the identity evidence was evicted.
  String? rootFor(String eventId) => _roots[eventId];

  final Map<String, String> _overlayRoots = {};

  /// Remembers which thread holds a reaction, edit or deletion, so a later
  /// removal of it (which names only its id) finds that thread.
  void recordOverlays(String rootId, Iterable<NostrEvent> overlays) {
    for (final event in overlays) {
      _overlayRoots.remove(event.id);
      _overlayRoots[event.id] = rootId;
      while (_overlayRoots.length > 8192) {
        _overlayRoots.remove(_overlayRoots.keys.first);
      }
    }
  }

  /// The thread holding overlay [eventId], or null when unknown or evicted.
  String? overlayRootFor(String eventId) => _overlayRoots[eventId];
}
