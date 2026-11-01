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

  /// The rooms pushed events may change: listed member rooms, not archived,
  /// not quiet (hidden DMs: docs/message-sync.md#push-inbox).
  Map<String, Channel> _pushFoldChannels() => {
    for (final channel in _shownList ?? const <Channel>[])
      if (channel.isMember &&
          !channel.isArchived &&
          !_hiddenDmIds.contains(channel.id))
        channel.id: channel,
  };

  bool _pushLeaseCurrent(MessageStoreLease lease) =>
      _lifecycleRef.mounted &&
      _lifecycleRef
              .read(messageStoreProvider)
              .lease(
                _memberSnapshotRelayBaseUrl ?? '',
                _memberSnapshotPubkey,
              ) ==
          lease;

  /// Folds the events the store accepted from the push inbox, under
  /// [lease]: the listed rooms' `lastMessageAt` moves at once (after the
  /// saved list is drawn), unread once read state is loaded, through
  /// [applyUnreadEvents] as the catch-up does. A list or account change
  /// meanwhile folds nothing.
  Future<void> _foldPushed(
    MessageStoreLease lease,
    List<NostrEvent> events,
  ) async {
    await _listSnapshot.pending;
    if (!_pushLeaseCurrent(lease)) return;
    final channels = _pushFoldChannels();
    final latest = <String, int>{};
    for (final event in events) {
      final channel = channels[event.channelId];
      if (channel == null ||
          channel.isForum ||
          !EventKind.channelMessageEventKinds.contains(event.kind)) {
        continue;
      }
      latest[channel.id] = max(latest[channel.id] ?? 0, event.createdAt);
    }
    _publishRecordedUnread(latest);
    final myPk = _lifecycleRef.read(myPubkeyProvider);
    if (myPk == null) return;
    final ReadStateState? readState;
    try {
      readState = await _loadedReadState();
    } catch (error) {
      debugPrint('[ChannelsNotifier] pushed unread skipped: $error');
      return;
    }
    if (readState == null || !_pushLeaseCurrent(lease)) return;
    applyUnreadEvents(
      events,
      readState,
      _pushFoldChannels(),
      myPk: myPk,
      mutedChannelIds: _mutedChannelIds(),
    );
  }
}
