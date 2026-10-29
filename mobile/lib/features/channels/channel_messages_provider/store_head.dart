part of '../channel_messages_provider.dart';

/// Opening the room from the device store when its newest window there is
/// certain (docs/message-sync.md#opening-a-room-from-the-store).
extension _StoreHead on ChannelMessagesNotifier {
  /// The newest window: on the room's first fetch, the store's window when it
  /// is certain; otherwise (and on every later fetch) the relay's.
  Future<List<NostrEvent>> _openWindowResponse(
    RelaySessionNotifier session,
  ) async {
    final filter = _channelWindowFilter(null);
    if (!_relayHistoryArrived) {
      final stored = await session.certainRoomHead(
        channelId,
        kinds: filter.kinds,
        limit: filter.limit,
      );
      if (stored != null) return stored;
    }
    return session.queryRelay([filter]);
  }
}
