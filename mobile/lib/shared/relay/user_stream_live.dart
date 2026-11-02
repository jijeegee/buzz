part of 'user_stream.dart';

/// Live ids remembered per connection for the new-row rule.
const _liveIdLimit = 50000;

/// How a stream frame's room relates to the room list (`routing`).
enum UserStreamRoute {
  /// A member room: store, open room, all-rooms consumers.
  member,

  /// An archived room: store and open room only.
  archived,

  /// A room the list does not know: store, and a throttled list refresh.
  unknown,
}

UserStreamRoute userStreamRoute(
  String? room, {
  required Set<String> memberRooms,
  required Set<String> archivedRooms,
}) {
  if (room != null && memberRooms.contains(room)) return UserStreamRoute.member;
  if (room != null && archivedRooms.contains(room)) {
    return UserStreamRoute.archived;
  }
  return UserStreamRoute.unknown;
}

/// List refreshes for unknown-room frames (`routingBurst`): one per burst
/// (frames within [delay] share it), a room asks again only [roomInterval]
/// after it last asked, and refreshes are at least [minGap] apart overall.
class UserStreamBurst {
  UserStreamBurst(
    this.refresh, {
    this.delay = const Duration(seconds: 1),
    this.roomInterval = const Duration(seconds: 10),
    this.minGap = const Duration(seconds: 5),
    Timer Function(Duration, void Function())? timer,
    int Function()? nowMs,
  }) : _timer = timer ?? Timer.new,
       _nowMs = nowMs ?? (() => DateTime.now().millisecondsSinceEpoch);

  final void Function() refresh;
  final Duration delay;
  final Duration roomInterval;
  final Duration minGap;
  final Timer Function(Duration, void Function()) _timer;
  final int Function() _nowMs;
  Timer? _pending;
  int? _lastRefreshMs;

  /// Room -> when it last asked for a refresh (ms).
  final _askedMs = <String, int>{};

  /// A frame of [room], which the room list does not know.
  void note([String? room]) {
    final now = _nowMs();
    final key = room ?? '';
    final asked = _askedMs[key];
    if (asked != null && now - asked < roomInterval.inMilliseconds) return;
    _askedMs.removeWhere((_, at) => now - at >= roomInterval.inMilliseconds);
    _askedMs[key] = now;
    if (_pending != null) return;
    var wait = delay;
    final last = _lastRefreshMs;
    if (last != null) {
      final gap = Duration(milliseconds: last + minGap.inMilliseconds - now);
      if (gap > wait) wait = gap;
    }
    _pending = _timer(wait, () {
      _pending = null;
      _lastRefreshMs = _nowMs();
      refresh();
    });
  }

  void dispose() {
    _pending?.cancel();
    _pending = null;
  }
}

/// What a `live`/`gated` connection carries after its EOSE.
class _Carry {
  _Carry(this.liveAt, this.captured, this.relayPubkey);

  /// The stream EOSE (seconds).
  final int liveAt;
  final int? captured;
  final String relayPubkey;

  /// Carried room -> when the stream started carrying it.
  final rooms = <String, int>{};
  late final span = UserStreamSpan(liveAt);

  /// Room -> the `created_at` of its last applied notice, and of its last
  /// 44101 still in force (frames newer than it are not shown).
  final noticeAt = <String, int>{};
  final removedAt = <String, int>{};
}

/// The stream's `live`/`gated` side: carried rooms, its coverage, the
/// catch-ups, membership notices and the hooks chunk catch-ups ask.
extension _UserStreamLive on UserStream {
  void _noteLiveId(_Connection connection, Object? id) {
    if (id is! String) return;
    final ids = connection.liveIds;
    if (ids.add(id) && ids.length > _liveIdLimit) ids.remove(ids.first);
  }

  /// A received EVENT (stream or chunk): remembered only while a stream catch-up may still ask
  /// (pending, carried, shadow, or a catch-up running); a connection whose
  /// stream is gone keeps none.
  void _noteReceivedId(_Connection connection, Object? id) {
    if (connection.phase == UserStreamPhase.none && connection.catchUps == 0) {
      if (connection.liveIds.isNotEmpty) connection.liveIds.clear();
      return;
    }
    _noteLiveId(connection, id);
  }

  /// An accepted EOSE at [nowSecs].
  void _startCatchUp(_Connection connection, UserStreamSnapshot snapshot) {
    final liveAt = host.nowMs() ~/ 1000;
    _lastSnapshot = snapshot;
    final captured = host.capture();
    final rooms = snapshot.rooms.keys.toList()..sort();
    UserStreamLivePrev? prev;
    if (snapshot.live) {
      final carry = connection.carry = _Carry(
        liveAt,
        captured,
        snapshot.relayPubkey,
      );
      for (final room in rooms) {
        carry.rooms[room] = liveAt;
      }
      host.extendCoverage(rooms, liveAt, liveAt, captured);
      prev = _livePrev;
      _saveLivePrev(connection);
    }
    final ids = connection.liveIds;
    connection.settled =
        _runCatchUp(
          connection,
          UserStreamCatchUpJob(
            mode: snapshot.mode.name,
            rooms: rooms,
            seqs: snapshot.rooms,
            epoch: snapshot.epoch,
            relayPubkey: snapshot.relayPubkey,
            liveAt: liveAt,
            captured: captured,
            // Spans this connection opened (chunk EOSEs, the stream's own
            // [L, L]) never stand for the offline gap.
            coverageBefore: min(connection.startedAt, liveAt),
            prev: prev,
            storedMark: connection.storedMark,
            stopped: () => connection.ended,
          ),
          ids.contains,
        ).then((result) {
          if (result == null || snapshot.live) return;
          connection.predicted.addAll([
            for (final MapEntry(:key, :value) in result.classes.entries)
              if (value == UserStreamRoomClass.equal) key,
          ]);
        });
  }

  /// Catch-ups of successive connections never overlap: a connection's
  /// waits for every earlier connection's, which stop at their next batch
  /// once their connection ended.
  Future<UserStreamCatchUpResult?> _runCatchUp(
    _Connection connection,
    UserStreamCatchUpJob job,
    bool Function(String id) receivedLive,
  ) async {
    final scope = _scope;
    connection.catchUps++;
    final earlier = [
      for (final (owner, running) in _catchUpsRunning)
        if (!identical(owner, connection)) running.future,
    ];
    final done = Completer<void>();
    _catchUpsRunning.add((connection, done));
    try {
      if (earlier.isNotEmpty) {
        await Future.wait(earlier);
        if (connection.ended) return null;
      }
      final result = await host.catchUp(job, receivedLive);
      // Counts of an earlier account never land in this one's totals.
      if (result != null && _scope == scope && _loaded) {
        _count((c) {
          for (final MapEntry(:key, :value) in result.counts.entries) {
            c.add(key, value);
          }
        });
      }
      return result;
    } catch (error) {
      debugPrint('[user-stream] catch-up stopped: $error');
      return null;
    } finally {
      done.complete();
      _catchUpsRunning.removeWhere((entry) => identical(entry.$2, done));
      connection.catchUps--;
    }
  }

  /// The previous live connection's set and bounds, for the next one's
  /// `liveMissed`; kept at the EOSE and with each span extension. The set
  /// is written only when it changed; otherwise only the bounds.
  void _saveLivePrev(_Connection connection) {
    final carry = connection.carry;
    final snapshot = connection.snapshot;
    if (carry == null || snapshot == null) return;
    final rooms = carry.rooms.keys.toSet();
    final prev = _livePrev = UserStreamLivePrev(
      rooms,
      snapshot.createdAt,
      connection.lastFrame ?? snapshot.createdAt,
    );
    if (!_writable) return;
    final same =
        _livePrevRooms != null &&
        _livePrevEose == prev.eoseAt &&
        setEquals(_livePrevRooms, rooms);
    if (same) {
      host.writeState({_liveLastKey: prev.encodeLast()});
      return;
    }
    _livePrevRooms = rooms;
    _livePrevEose = prev.eoseAt;
    host.writeState({
      _livePrevKey: prev.encode(),
      _liveLastKey: prev.encodeLast(),
    });
  }

  /// A stream frame after a live EOSE: grows the stream's span (at most
  /// every 60 s), and tells whether it may be shown.
  bool _carryFrame(_Connection connection, Map<String, dynamic> event) {
    final carry = connection.carry;
    if (carry == null) return true;
    final now = host.nowMs() ~/ 1000;
    if (carry.span.noteFrame(now)) {
      final byStart = <int, List<String>>{};
      for (final MapEntry(key: room, value: since) in carry.rooms.entries) {
        byStart.putIfAbsent(since, () => []).add(room);
      }
      for (final MapEntry(key: since, value: rooms) in byStart.entries) {
        host.extendCoverage(rooms, since, now, carry.captured);
      }
      _saveLivePrev(connection);
    }
    final room = _roomOf(event);
    final removed = room == null ? null : carry.removedAt[room];
    final createdAt = event['created_at'];
    return removed == null || createdAt is! int || createdAt <= removed;
  }

  /// The mid-connection fallback: the carried rooms go back to chunks and
  /// their store catches up to the CLOSED time [closedAt].
  void _fallBack(_Connection connection, int closedAt) {
    final carry = connection.carry;
    final snapshot = connection.snapshot;
    connection.carry = null;
    if (carry == null || snapshot == null) return;
    final rooms = carry.rooms.keys.toList()..sort();
    final ids = connection.liveIds;
    connection.settled = _runCatchUp(
      connection,
      UserStreamCatchUpJob(
        mode: snapshot.mode.name,
        rooms: rooms,
        seqs: snapshot.rooms,
        epoch: snapshot.epoch,
        relayPubkey: snapshot.relayPubkey,
        liveAt: closedAt,
        captured: host.capture(),
        fallback: true,
        storedMark: connection.storedMark,
        stopped: () => connection.ended,
      ),
      ids.contains,
    );
  }
}

extension UserStreamCarried on UserStream {
  /// Rooms the stream carries now: none unless an accepted `live`/`gated`
  /// EOSE, and none from an overflow or `lagged` CLOSED on (the relay
  /// dropped frames; see [UserStream.closingSocket]).
  Set<String> get carriedRooms {
    final connection = _connection;
    final carry = connection?.carry;
    if (carry == null) return const {};
    return carry.rooms.keys.toSet();
  }

  /// When the stream started carrying [room] on this connection, if it does.
  int? liveAtOf(String room) => _connection?.carry?.rooms[room];

  /// Completes once the current connection's stream catch-up has ended.
  Future<void>? get settled => _connection?.settled;

  /// Whether [id] was received live on this connection (stream or chunk).
  bool receivedLive(String id) => _connection?.liveIds.contains(id) ?? false;

  /// An EVENT on a live chunk of this connection.
  void noteLiveEvent(Object? id) {
    final connection = _connection;
    if (connection != null) _noteReceivedId(connection, id);
  }

  /// Hears each stored stream frame and whether it may be shown.
  void Function() addFrameListener(
    void Function(Map<String, dynamic> event, bool render) listener,
  ) {
    _frameListeners.add(listener);
    return () => _frameListeners.remove(listener);
  }

  /// A 44100 ([added]) or 44101 notice for [room] from [pubkey], created at
  /// [createdAt] and received at [receivedAt] (`membership`).
  void noteMembership({
    required bool added,
    required String room,
    required int createdAt,
    required String pubkey,
    required int receivedAt,
  }) {
    final connection = _connection;
    if (connection == null) return;
    if (connection.phase == UserStreamPhase.shadow) {
      if (added) connection.comparator?.joined.add(room);
      return;
    }
    final carry = connection.carry;
    if (connection.phase != UserStreamPhase.live || carry == null) return;
    if (pubkey.toLowerCase() != carry.relayPubkey.toLowerCase()) return;
    // A notice older than the snapshot (a reconnect catch-up replaying the
    // gap) is already reflected in it.
    if (createdAt < (connection.snapshot?.createdAt ?? 0)) return;
    final last = carry.noticeAt[room];
    if (last != null && createdAt < last) return;
    carry.noticeAt[room] = createdAt;
    if (!added) {
      carry.removedAt[room] = createdAt;
      if (carry.rooms.remove(room) != null) _notify();
      return;
    }
    carry.removedAt.remove(room);
    if (carry.rooms.containsKey(room)) return;
    carry.rooms[room] = receivedAt;
    host.extendCoverage([room], receivedAt, receivedAt, host.capture());
    _notify();
  }
}

/// The stream's say in chunk catch-ups (docs/message-sync.md#handoff,
/// docs/message-sync.md#watermark-room_seq).
class _UserStreamHooks implements StoreStreamHooks {
  _UserStreamHooks(this.stream);

  final UserStream stream;

  @override
  Set<String> streamOwnedRooms() {
    final connection = stream._connection;
    if (connection?.phase != UserStreamPhase.live) return const {};
    return connection!.snapshot?.rooms.keys.toSet() ?? const {};
  }

  @override
  StoreSeqVouch? vouch(int generation, List<String> rooms) {
    final connection = stream._connection;
    final snapshot = connection?.snapshot;
    if (connection == null ||
        snapshot == null ||
        connection.generation != generation) {
      return null;
    }
    final seqs = {for (final room in rooms) room: ?snapshot.rooms[room]};
    return seqs.isEmpty ? null : StoreSeqVouch(seqs, snapshot.epoch);
  }

  @override
  Future<int?> storedMark(int generation) async {
    final connection = stream._connection;
    if (connection == null || connection.generation != generation) return null;
    return connection.storedMark;
  }

  @override
  void walkEnded(
    StoreSeqVouch? vouch,
    List<String> rooms,
    int liveAt,
    StoreWalk walk,
    Map<String, (int, int)> heads,
  ) {
    if (!walk.completed) return;
    if (vouch == null) {
      final snapshot = stream._lastSnapshot;
      if (snapshot == null) return;
      final skipped = rooms.where(snapshot.rooms.containsKey).length;
      if (skipped > 0) {
        stream._count((c) => c.add('seqWriteSkippedOrder', skipped));
      }
      return;
    }
    final predicted = stream._connection?.predicted;
    if (predicted == null || predicted.isEmpty) return;
    final perRoom = <String, int>{};
    for (final row in userStreamNewRows(
      walk,
      heads,
      liveAt,
      stream.receivedLive,
    )) {
      perRoom[row.room] = (perRoom[row.room] ?? 0) + 1;
    }
    final checked = rooms.where(predicted.remove).toList();
    final skips = userStreamSkipVerification([
      for (final room in checked) (true, perRoom[room] ?? 0),
    ]);
    stream._count(
      (c) => c
        ..add('verifiedSkips', skips.verified)
        ..add('falseSkip', skips.falseSkip),
    );
  }
}
