part of '../channel_messages_provider.dart';

/// What this room keeps on the device: the persisted head it paints from
/// before the relay answers, and the message database every fetched page and
/// live event is written to. Reads still come from the head only.
extension _LocalStore on ChannelMessagesNotifier {
  /// The room's own live subscription: stored, then shown.
  void _onSubscriptionEvent(NostrEvent event) {
    recordLoadedEvents(_storeRef, [event]);
    _handleLiveEvent(event);
  }

  /// Paints the persisted newest page while the relay query is in flight.
  /// Relay-loaded or live state always wins, and the next relay page replaces
  /// the cached rows.
  Future<void> _restoreCachedHead(ChannelHeadScope scope) async {
    try {
      final events = await _storeRef
          .read(channelHeadCacheProvider)
          .load(scope, channelId);
      if (events == null ||
          !_storeRef.mounted ||
          _relayHistoryArrived ||
          _lastKnownMessages != null ||
          _readHeadScope() != scope) {
        return;
      }
      _windowStore = replaceNewestChannelWindow(
        _windowStore,
        parseChannelWindowResponse(events, channelId, null),
      );
      final messages = flattenChannelWindowEvents(_windowStore);
      _cachedHeadIds = {for (final event in messages) event.id};
      _lastKnownMessages = messages;
      _publishSummaryChange();
    } catch (error) {
      debugPrint(
        '[ChannelMessagesNotifier] cached head unavailable for $channelId: $error',
      );
    }
  }

  ChannelHeadScope? _readHeadScope() {
    try {
      return _storeRef.read(channelHeadScopeProvider);
    } catch (_) {
      return null;
    }
  }

  /// Stores the newest page: the head the room paints from, and its rows
  /// with their coverage in the message database.
  void _persistHead(List<NostrEvent> response, ChannelWindowPage page) {
    recordChannelWindowPage(_storeRef, channelId, response, page);
    final scope = _readHeadScope();
    if (scope == null) return;
    _storeRef.read(channelRoomStoreProvider).clearGap(scope, channelId);
    unawaited(
      _storeRef
          .read(channelHeadCacheProvider)
          .store(scope, channelId, response)
          .catchError((Object error) {
            debugPrint(
              '[ChannelMessagesNotifier] failed to persist head for $channelId: $error',
            );
          }),
    );
  }
}
