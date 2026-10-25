part of 'channels_provider.dart';

/// What the channel list's live subscription asks for: room activity, plus
/// thread summaries so stored rows keep their reply counts.
const _globalLiveEventKinds = [
  ...EventKind.channelEventKinds,
  EventKind.channelThreadSummary,
];

/// Routes the channel list's live events: every joined room's event is
/// stored (open or not), then room activity updates the list and its
/// listeners.
extension _ChannelsNotifierGlobalFeed on ChannelsNotifier {
  /// An event already delivered (a catch-up overlapping the live feed) is
  /// skipped.
  void _ingestLiveEvent(NostrEvent event) {
    final scope = _lifecycleRef.read(messageScopeProvider);
    if (scope != null) {
      if (!_noteDelivered(scope, event)) return;
      _lifecycleRef.read(channelRoomStoreProvider).ingest(scope, event);
    }
    // Summaries only describe threads; the list and its readers ignore them.
    if (event.kind == EventKind.channelThreadSummary) return;
    _handleLiveEvent(event);
    _lifecycleRef.read(channelLiveEventsProvider).emit(event);
  }
}
