part of 'channels_provider.dart';

extension _UnreadEvents on ChannelsNotifier {
  /// Records [events] newer than their channel's read marker as unread, for
  /// the channels in [channelById], and advances those channels'
  /// `lastMessageAt` as live events do. Returns the newest recorded
  /// `created_at` per channel (empty: nothing changed). Shared by the unread
  /// catch-up and the push inbox fold.
  Map<String, int> applyUnreadEvents(
    Iterable<NostrEvent> events,
    ReadStateState readState,
    Map<String, Channel> channelById, {
    required String myPk,
    required Set<String> mutedChannelIds,
  }) {
    final recordedAtByChannel = <String, int>{};
    for (final event in events) {
      final channelId = event.channelId;
      if (channelId == null) continue;
      final channel = channelById[channelId];
      if (channel == null) continue;
      final readAt = readState.effectiveTimestamp(channelId);
      if (event.pubkey.toLowerCase() == myPk.toLowerCase()) continue;
      if (readAt != null && event.createdAt <= readAt) continue;
      if (!shouldNotifyForEvent(
        event,
        myPk,
        participatedRootIds: _participatedRootIds,
        followedRootIds: _followedRootIds(),
        authoredRootIds: _authoredRootIds,
        mutedChannelIds: mutedChannelIds,
        channelId: channel.id,
      )) {
        continue;
      }
      _recordUnreadEvent(channel, event, myPk);
      recordedAtByChannel[channelId] = max(
        recordedAtByChannel[channelId] ?? 0,
        event.createdAt,
      );
    }
    _publishRecordedUnread(recordedAtByChannel);
    return recordedAtByChannel;
  }
}
