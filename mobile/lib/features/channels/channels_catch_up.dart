part of 'channels_provider.dart';

/// How far a catch-up reaches back before the watermark, for events that
/// arrive slightly out of order.
const _catchUpSkewSeconds = 60;

/// A watermark older than this is not caught up: every room is marked as
/// possibly missing rows and reloads its newest page when it is next read.
const _catchUpMaxAgeSeconds = 24 * 60 * 60;

/// The relay's cap on a filter's limit.
const _catchUpPageLimit = 1000;

/// Pages read per 128-room chunk before its rooms count as unfilled.
const _catchUpMaxPages = 3;

/// Event ids remembered to skip what the live feed already delivered.
const _deliveredIdLimit = 4096;

/// This device's clock in Unix seconds, replaceable in tests.
@visibleForTesting
int Function() roomCatchUpClock = _unixNow;

int _unixNow() => DateTime.now().millisecondsSinceEpoch ~/ 1000;

/// What the reconnect catch-up of the channel list's live feed remembers.
class _RoomCatchUpState {
  /// Per relay and account: the time through which every live event of the
  /// fed rooms is known to have arrived. Never ahead of this device's clock.
  /// Stored in the message database (sync_state, by physical server) with
  /// the events it vouches for; see [ChannelRoomStore.advanceWatermark].
  final Map<MessageScope, int> watermarks = {};

  /// Each scope's read of its stored watermark, once per session.
  final Map<MessageScope, Future<void>> loads = {};
  final Set<MessageScope> loaded = {};

  /// When each scope's feed first subscribed this session, and the
  /// [pendingEpoch] then: with nothing stored, the watermark starts there.
  final Map<MessageScope, (int, int)> feedStarts = {};

  /// The scope whose feed was interrupted and is not caught up yet.
  MessageScope? pending;

  /// Counts interruptions, so a run knows whether another one happened
  /// while it was fetching.
  int pendingEpoch = 0;
  bool running = false;

  final Set<String> deliveredIds = {};
  ChannelRoomStore? store;
}

/// Reconnect recovery for every room the channel list's live feed covers:
/// one "everything since the watermark" query per 128 rooms, merged into the
/// room stores and handed to open rooms, instead of each room reloading.
extension _ChannelsNotifierCatchUp on ChannelsNotifier {
  MessageScope? get _catchUpScope => _lifecycleRef.read(messageScopeProvider);

  /// Records [event] as delivered; false when it already was. Advances the
  /// watermark while the feed is uninterrupted.
  bool _noteDelivered(MessageScope scope, NostrEvent event) {
    final ids = _catchUp.deliveredIds;
    if (!ids.add(event.id)) return false;
    if (ids.length > _deliveredIdLimit) ids.remove(ids.first);
    // A future timestamp cannot vouch for anything not yet received.
    _advanceWatermark(scope, min(event.createdAt, roomCatchUpClock()));
    return true;
  }

  int? _watermark(MessageScope scope) => _catchUp.watermarks[scope];

  /// Reads [scope]'s stored watermark (once per session).
  Future<void> _loadWatermark(MessageScope scope) {
    final state = _catchUp;
    final store = _lifecycleRef.read(channelRoomStoreProvider);
    return state.loads[scope] ??= () async {
      final stored = await store.loadWatermark(scope);
      state.loaded.add(scope);
      if (stored != null) {
        state.watermarks[scope] = max(
          state.watermarks[scope] ?? stored,
          stored,
        );
      }
    }();
  }

  /// The feed now starts with nothing to catch up from, unless [scope] has a
  /// watermark from before (this session or a stored one).
  Future<void> _startWatermark(MessageScope scope) async {
    final state = _catchUp;
    final (startedAt, epoch) = state.feedStarts[scope] ??= (
      roomCatchUpClock(),
      state.pendingEpoch,
    );
    await _loadWatermark(scope);
    if (state.watermarks[scope] != null) return;
    state.watermarks[scope] = startedAt;
    // Nothing stored vouches for this server: ranges kept through another
    // server (or an older install) end before its first watermark.
    state.store
      ?..resetLiveRanges(scope)
      ..advanceWatermark(scope, startedAt);
    // A catch-up marked before the feed started has nothing to catch up
    // from; one marked since (a reconnect) catches up from [startedAt].
    if (state.pending == scope && state.pendingEpoch == epoch) {
      state.pending = null;
    }
  }

  /// Moves the watermark to [at]; ignored while a catch-up is owed, since
  /// nothing after the interruption vouches for what it missed.
  void _advanceWatermark(MessageScope scope, int at, {bool caughtUp = false}) {
    final state = _catchUp;
    if (!caughtUp && state.pending == scope) return;
    final current = state.watermarks[scope];
    if (current == null || at <= current) return;
    state.watermarks[scope] = at;
    state.store?.advanceWatermark(scope, at);
  }

  /// A relay round trip that started at [at] answered over the live socket,
  /// so the feed delivered everything sent before it.
  void _noteFeedAlive(int at) {
    final scope = _catchUpScope;
    if (scope == null || _catchUp.watermarks[scope] == null) return;
    _advanceWatermark(scope, at);
  }

  /// The feed for the current scope was interrupted (a reconnect, or the
  /// list starting again for a scope seen before): catch it up once its
  /// subscriptions are back.
  /// Until the stored watermark is read this assumes one exists;
  /// [_startWatermark] drops the catch-up when none does.
  void _markCatchUpPending() {
    final scope = _catchUpScope;
    if (scope == null) return;
    final state = _catchUp;
    if (state.loaded.contains(scope) && _watermark(scope) == null) {
      // Read, and nothing stored: the feed starts fresh.
      return;
    }
    state.store ??= _lifecycleRef.read(channelRoomStoreProvider);
    unawaited(_loadWatermark(scope));
    state
      ..pending = scope
      ..pendingEpoch += 1;
  }

  /// Rooms with an established live chunk.
  Set<String> _fedRoomIds() => {
    for (final entry in _liveSubscriptionsByChunk.entries)
      if (entry.value.unsubscribe != null)
        for (final id in entry.key.split('\u0000'))
          if (_desiredLiveChannelIds.contains(id)) id,
  };

  void _publishFedRooms() {
    final store = _lifecycleRef.read(channelRoomStoreProvider);
    _catchUp.store = store;
    store.setFedRooms(_catchUpScope, _fedRoomIds());
  }

  /// Forgets the feed's rooms; safe while the notifier is disposing.
  void _clearCatchUpFeed() {
    _catchUp
      ..pending = null
      ..store?.setFedRooms(null, const {});
  }

  /// Runs the owed catch-up for the current scope, again if the feed is
  /// interrupted while one is in flight.
  Future<void> _runRoomCatchUp() async {
    final state = _catchUp;
    final scope = _catchUpScope;
    if (scope == null || state.pending != scope || state.running) return;
    state.running = true;
    try {
      // The stored watermark decides whether anything is owed at all.
      await _startWatermark(scope);
      while (_lifecycleRef.mounted &&
          state.pending == scope &&
          _catchUpScope == scope &&
          _lifecycleRef.read(relaySessionProvider).status ==
              SessionStatus.connected) {
        final epoch = state.pendingEpoch;
        if (!await _catchUpOnce(scope)) return;
        if (state.pendingEpoch == epoch) state.pending = null;
      }
    } finally {
      state.running = false;
    }
  }

  /// Returns false when the run was abandoned (disconnect, scope switch).
  Future<bool> _catchUpOnce(MessageScope scope) async {
    final rooms = _fedRoomIds();
    final startedAt = roomCatchUpClock();
    final watermark = _watermark(scope) ?? startedAt;
    final since = max(0, watermark - _catchUpSkewSeconds);
    final gaps = <String>{};
    final fetched = <NostrEvent>[];
    if (startedAt - watermark > _catchUpMaxAgeSeconds) {
      gaps.addAll(rooms);
    } else {
      final session = _lifecycleRef.read(relaySessionProvider.notifier);
      for (final chunk in chunkChannelIdsForLiveSubscriptions(rooms)) {
        try {
          final (events, complete) = await _fetchCatchUpChunk(
            session,
            chunk,
            since,
          );
          fetched.addAll(events);
          if (!complete) gaps.addAll(chunk);
        } catch (error) {
          debugPrint('[ChannelsNotifier] room catch-up failed: $error');
          gaps.addAll(chunk);
        }
        if (!_isCatchUpCurrent(scope)) return false;
      }
    }
    await _applyCatchUp(scope, rooms, gaps, fetched, since, startedAt);
    return _isCatchUpCurrent(scope);
  }

  bool _isCatchUpCurrent(MessageScope scope) =>
      _lifecycleRef.mounted &&
      _catchUpScope == scope &&
      _lifecycleRef.read(relaySessionProvider).status ==
          SessionStatus.connected;

  /// Every event of [rooms] since [since], newest first, paging back from
  /// the oldest row while pages come back full. The second value says
  /// whether the pages reached [since].
  Future<(List<NostrEvent>, bool)> _fetchCatchUpChunk(
    RelaySessionNotifier session,
    List<String> rooms,
    int since,
  ) async {
    final events = <NostrEvent>[];
    NostrEvent? cursor;
    for (var page = 0; page < _catchUpMaxPages; page++) {
      final rows = await session.queryRelay([
        NostrFilter(
          kinds: _globalLiveEventKinds,
          tags: {'#h': rooms},
          since: since,
          until: cursor?.createdAt,
          limit: _catchUpPageLimit,
          extensions: {if (cursor != null) 'before_id': cursor.id},
        ),
      ]);
      events.addAll(rows);
      if (rows.length < _catchUpPageLimit) return (events, true);
      // The relay orders by time, newest first, then by id ascending.
      cursor = rows.reduce(
        (oldest, row) =>
            row.createdAt < oldest.createdAt ||
                (row.createdAt == oldest.createdAt &&
                    row.id.compareTo(oldest.id) > 0)
            ? row
            : oldest,
      );
    }
    return (events, false);
  }

  Future<void> _applyCatchUp(
    MessageScope scope,
    Set<String> rooms,
    Set<String> gaps,
    List<NostrEvent> fetched,
    int since,
    int startedAt,
  ) async {
    final store = _lifecycleRef.read(channelRoomStoreProvider);
    fetched.sort(
      (a, b) => a.createdAt != b.createdAt
          ? a.createdAt.compareTo(b.createdAt)
          : a.id.compareTo(b.id),
    );
    final byRoom = <String, List<NostrEvent>>{};
    final seen = <String>{};
    for (final event in fetched) {
      final room = event.channelId;
      if (room == null || !rooms.contains(room) || !seen.add(event.id)) {
        continue;
      }
      (byRoom[room] ??= []).add(event);
      if (_desiredLiveChannelIds.contains(room)) {
        _ingestLiveEvent(event);
      }
    }
    // The events, the unfilled rooms' holes and the watermark land in one
    // transaction.
    _advanceWatermark(scope, startedAt, caughtUp: true);
    var unfilled = gaps;
    if (!await store.commitCatchUp(
      scope,
      rooms: rooms,
      gaps: gaps,
      watermark: startedAt,
    )) {
      // Nothing of it was stored: any room may be missing rows.
      unfilled = rooms;
      store.markGap(scope, rooms);
    }
    if (!_lifecycleRef.mounted || _catchUpScope != scope) return;
    store.publishCatchUp(
      RoomCatchUp(scope: scope, rooms: rooms, gaps: unfilled, events: byRoom),
    );
  }
}
