part of 'channels_provider.dart';

/// The room list's side of the user stream (docs/message-sync.md#handoff):
/// in `live`/`gated` the carried rooms leave the chunks after the stream
/// EOSE, stream frames are routed like a chunk's, and 44100/44101 notices
/// change what the stream carries.
class _StreamGlue {
  _StreamGlue(this.stream);

  final UserStream stream;
  final stops = <void Function()>[];
  Future<void Function()>? membership;
  UserStreamBurst? burst;
  Set<String> carried = const {};

  /// The stream carried rooms since the last connect.
  bool reopen = false;

  /// The arguments of the last `_subscribeLive`, to re-sync the chunks.
  List<Channel>? channels;
  _ChannelRefreshFence? fence;

  void dispose() {
    for (final stop in stops) {
      stop();
    }
    stops.clear();
    burst?.dispose();
    unawaited(membership?.then((stop) => stop(), onError: (_) {}));
    membership = null;
  }
}

final _streamGlues = Expando<_StreamGlue>();

const _memberAddedKind = 44100;
const _memberRemovedKind = 44101;

extension _ChannelsUserStream on ChannelsNotifier {
  /// Rooms the stream carries now (none unless `live`/`gated` after EOSE).
  Set<String> get _streamCarried => _streamGlues[this]?.carried ?? const {};

  /// Rooms the chunks carry: member ∪ quiet rooms minus the stream's.
  Set<String> get _chunkRoomIds {
    final carried = _streamCarried;
    if (carried.isEmpty) return _liveChunkRoomIds;
    return _liveChunkRoomIds.difference(carried);
  }

  /// Hooks the stream once per build and remembers [channels]/[fence] for
  /// re-syncs on stream changes.
  ///
  /// [carried]: tests stand in for the stream's carried set.
  void _attachUserStream(
    List<Channel> channels,
    _ChannelRefreshFence fence, [
    Set<String>? carried,
  ]) {
    var glue = _streamGlues[this];
    final attachedLate = glue == null;
    if (glue == null) {
      final stream = _lifecycleRef
          .read(relaySessionProvider.notifier)
          .userStream;
      glue = _streamGlues[this] = _StreamGlue(stream);
      final attached = glue;
      attached
        ..burst = UserStreamBurst(() {
          if (_lifecycleRef.mounted) unawaited(_backstopRefresh());
        })
        ..stops.add(stream.addListener(() => _onStreamChanged(attached)))
        ..stops.add(stream.addFrameListener(_onStreamFrame));
      attached.carried = carried ?? stream.carriedRooms;
    }
    glue
      ..channels = channels
      ..fence = fence;
    // The stream's EOSE came before this build.
    if (attachedLate) _noteCarriedRooms(glue, glue.carried);
  }

  /// The stream carries [grown] on top of what it carried before.
  void _noteCarriedRooms(_StreamGlue glue, Set<String> grown) {
    if (glue.carried.isNotEmpty) {
      glue.reopen = true;
      _listenMembership(glue);
    }
    // The quiet-room backfill waits for the stream EOSE, not the chunks'.
    final quietReady = grown.where(_quietLiveChannelIds.contains);
    if (quietReady.isNotEmpty) {
      _lifecycleRef.read(roomLiveRouterProvider).markRoomsReady(quietReady);
    }
  }

  /// [carried]: tests stand in for the stream's carried set.
  void _onStreamChanged(_StreamGlue glue, [Set<String>? carried]) {
    if (!_lifecycleRef.mounted || !identical(_streamGlues[this], glue)) return;
    final before = glue.carried;
    final now = glue.carried = carried ?? glue.stream.carriedRooms;
    if (setEquals(before, now)) return;
    _noteCarriedRooms(glue, now.difference(before));
    // The router stops counting rooms the stream no longer carries at once.
    _publishLiveCoverage();
    // An overflow or `lagged` CLOSED: the socket closes within 2 s and the
    // chunks open on the next connection (glue.reopen), not on this one.
    if (glue.stream.closingSocket) return;
    final channels = glue.channels;
    final fence = glue.fence;
    if (channels == null || fence == null || !fence.isCurrent) return;
    // Rooms back on chunks may have missed unread rows; a hand-off does not.
    unawaited(
      _subscribeLive(
        channels,
        fence,
        unreadCatchUp: before.difference(now).isNotEmpty,
      ),
    );
  }

  /// 44100/44101 for the account, on the addressed bundle, once the stream
  /// first carries rooms (no extra REQ without the stream).
  void _listenMembership(_StreamGlue glue) {
    if (glue.membership != null) return;
    final me = _lifecycleRef.read(myPubkeyProvider)?.toLowerCase();
    if (me == null) return;
    final session = _lifecycleRef.read(relaySessionProvider.notifier);
    final membership = glue.membership = session.subscribe(
      NostrFilter(
        kinds: const [_memberAddedKind, _memberRemovedKind],
        tags: {
          '#p': [me],
        },
        limit: 0,
      ),
      (event) {
        final room = event.channelId;
        if (room == null) return;
        glue.stream.noteMembership(
          added: event.kind == _memberAddedKind,
          room: room,
          createdAt: event.createdAt,
          pubkey: event.pubkey,
          receivedAt: DateTime.now().millisecondsSinceEpoch ~/ 1000,
        );
      },
    );
    membership.catchError((Object error) {
      if (identical(glue.membership, membership)) glue.membership = null;
      debugPrint('[ChannelsNotifier] membership notices failed: $error');
      return () {};
    });
  }

  /// A stored stream frame (`routing`): member rooms reach everyone as on a
  /// chunk, archived and quiet rooms the open room only, an unknown room
  /// refreshes the list (throttled, [UserStreamBurst]). Frames hidden by a
  /// 44101 are only stored.
  void _onStreamFrame(Map<String, dynamic> json, bool render) {
    if (!render || !_lifecycleRef.mounted) return;
    final NostrEvent event;
    try {
      event = NostrEvent.fromJson(json);
    } catch (_) {
      return;
    }
    // Every frame goes to the router, as on a chunk: reactions and their
    // removals have no `h`, and an open room may not be listed yet.
    _lifecycleRef
        .read(roomLiveRouterProvider)
        .routeStreamEvent(event, _streamCarried);
    final room = event.channelId;
    if (room == null) return;
    final route = userStreamRoute(
      room,
      memberRooms: _desiredLiveChannelIds,
      archivedRooms: {
        for (final channel in _shownList ?? const <Channel>[])
          if (channel.isArchived) channel.id,
      },
    );
    final quiet = _quietLiveChannelIds.contains(room);
    if (route == UserStreamRoute.unknown && !quiet) {
      _streamGlues[this]?.burst?.note(room);
      return;
    }
    if (route != UserStreamRoute.member || !reachesAllRoomListeners(event)) {
      return;
    }
    _handleLiveEvent(event);
    _lifecycleRef.read(channelLiveEventsProvider).emit(event);
  }

  /// A new connection after one whose stream carried rooms: their chunks
  /// open at once, before the list refresh (docs/message-sync.md#handoff).
  void _reopenStreamChunks() {
    final glue = _streamGlues[this];
    if (glue == null || !glue.reopen) return;
    glue.reopen = false;
    final channels = glue.channels;
    final fence = glue.fence;
    if (channels == null || fence == null || !fence.isCurrent) return;
    unawaited(_subscribeLive(channels, fence, unreadCatchUp: false));
  }

  void _detachUserStream() {
    _streamGlues[this]?.dispose();
    _streamGlues[this] = null;
  }
}

extension ChannelsUserStreamTesting on ChannelsNotifier {
  /// Tests: the stream now carries [carried] (a hand-off at its EOSE).
  @visibleForTesting
  void debugStreamCarried(Set<String> carried) {
    final glue = _streamGlues[this];
    if (glue != null) _onStreamChanged(glue, carried);
  }

  /// Tests: a build attaches to a stream already carrying [carried].
  @visibleForTesting
  void debugStreamAttachedLate(Set<String> carried) {
    final glue = _streamGlues[this];
    final channels = glue?.channels;
    final fence = glue?.fence;
    if (channels == null || fence == null) return;
    _detachUserStream();
    _attachUserStream(channels, fence, carried);
  }

  /// Tests: a stored stream frame.
  @visibleForTesting
  void debugStreamFrame(NostrEvent event) =>
      _onStreamFrame(event.toJson(), true);

  /// Tests: the membership listen is open.
  @visibleForTesting
  bool get debugStreamListensMembership =>
      _streamGlues[this]?.membership != null;

  /// Tests: the next connection reopens the stream's rooms on chunks.
  @visibleForTesting
  bool get debugStreamReopens => _streamGlues[this]?.reopen ?? false;
}
