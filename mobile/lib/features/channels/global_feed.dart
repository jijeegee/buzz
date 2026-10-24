part of 'channels_provider.dart';

/// What the channel list's live subscription asks for: room activity, plus
/// thread summaries so stored tails keep their reply counts.
const _globalLiveEventKinds = [
  ...EventKind.channelEventKinds,
  EventKind.channelThreadSummary,
];

/// Routes the channel list's live events: every joined room's event goes to
/// its stored tail (open or not), then room activity updates the list and
/// its listeners.
extension _ChannelsNotifierGlobalFeed on ChannelsNotifier {
  /// [liveSince] is when the delivering feed became continuous. An event
  /// already delivered (a catch-up overlapping the live feed) is skipped.
  void _ingestLiveEvent(NostrEvent event, {required int liveSince}) {
    final scope = _lifecycleRef.read(channelHeadScopeProvider);
    if (scope != null) {
      if (!_noteDelivered(scope, event)) return;
      _lifecycleRef
          .read(channelRoomStoreProvider)
          .ingest(scope, event, liveSince: liveSince);
    }
    // Summaries only describe threads; the list and its readers ignore them.
    if (event.kind == EventKind.channelThreadSummary) return;
    _handleLiveEvent(event);
    _lifecycleRef.read(channelLiveEventsProvider).emit(event);
  }
}
