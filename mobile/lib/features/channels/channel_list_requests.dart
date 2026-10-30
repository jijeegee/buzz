part of 'channels_provider.dart';

/// What a channel-list refresh asks for once it has the membership list.
typedef _MembershipParts = ({
  List<NostrEvent> memberMetas,
  List<NostrEvent>? directory,
  Set<String> hiddenDmIds,
  List<NostrEvent> memberEvents,
  List<NostrEvent> huddleStarts,
});

/// The channel-list requests that need only the membership list go out at
/// once rather than one after another (docs/message-sync.md#launch-order).
extension _ChannelListRequests on ChannelsNotifier {
  /// Joined metadata, the open directory (when [fetchDirectory]), hidden
  /// DMs, the membership snapshots of [memberIds] and their Huddle starts.
  /// The relay filters the global kind:39000 directory query by the
  /// caller's access; private channels and DMs are still rejected by the
  /// caller so discovery fails closed if that contract ever regresses. The
  /// relay-signed kind:39000 metadata identifies the relay, not the channel
  /// creator; the owner role in the kind:39002 snapshots is the canonical
  /// creator identity used to reject forged Huddle links.
  Future<_MembershipParts> _fetchMembershipParts(
    RelaySessionNotifier session,
    _ChannelRefreshFence fence,
    List<String> memberIds,
    String myPk, {
    required bool fetchDirectory,
  }) async {
    const none = <NostrEvent>[];
    final parts = await _fenced(
      fence,
      Future.wait<Object?>([
        memberIds.isEmpty
            ? Future.value(none)
            : session.fetchHistory(NostrFilters.channelMetadata(memberIds)),
        fetchDirectory
            ? _refreshCoordinator.loadDirectory(session, fence)
            : Future.value(),
        _fetchHiddenDmIds(session, myPk),
        memberIds.isEmpty
            ? Future.value(none)
            : session.fetchHistory(
                NostrFilter(
                  kinds: const [39002],
                  tags: {'#d': memberIds},
                  limit: memberIds.length,
                ),
              ),
        memberIds.isEmpty
            ? Future.value(none)
            : _fetchHuddleStarts(session, memberIds),
      ]),
    );
    return (
      memberMetas: parts[0] as List<NostrEvent>,
      directory: parts[1] as List<NostrEvent>?,
      hiddenDmIds: parts[2] as Set<String>,
      memberEvents: parts[3] as List<NostrEvent>,
      huddleStarts: parts[4] as List<NostrEvent>,
    );
  }
}
