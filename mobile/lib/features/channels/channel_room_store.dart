import 'dart:async';
import 'dart:math';

import 'package:flutter/foundation.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:sqlite3/sqlite3.dart';

import '../../shared/message_db/coverage.dart';
import '../../shared/message_db/event_rows.dart';
import '../../shared/message_db/message_db.dart';
import '../../shared/message_db/message_reads.dart';
import '../../shared/message_db/message_store.dart';
import '../../shared/relay/relay.dart';

/// Whether [event] is a row of a room's main timeline.
bool isMainTimelineRow(NostrEvent event) =>
    EventKind.channelTimelineContentKinds.contains(event.kind) &&
    event.threadReference.parentId == null;

/// The one entry point for storing received events on this device. Live
/// events are collected and written in batches, so a busy feed costs one
/// transaction per account (message database) per [debounce]. Fetched pages go to the message
/// database at once, with the coverage they prove ([recordPage]).
///
/// It also keeps the channel list's catch-up state in the database: each
/// physical server's watermark, and each fed room's live range (see
/// coverage.dart). A watermark is written only in the transaction that
/// stores the events it vouches for, together with the end of every live
/// range those events could not keep complete.
class ChannelRoomStore {
  ChannelRoomStore({
    MessageStore? messages,
    this.debounce = const Duration(seconds: 2),
    int Function()? clock,
  }) : _messages = messages,
       _clock = clock ?? _nowSeconds;

  /// Pending events that force a write before [debounce] runs out.
  static const maxPendingEvents = 500;

  final MessageStore? _messages;
  final Duration debounce;
  final int Function() _clock;

  /// Rows waiting for the message database, per account, with the store
  /// generation they were queued under (a clear since drops them).
  Map<MessageScope, ({int generation, List<MessageRow> rows})> _dbPending = {};
  int _dbPendingCount = 0;

  /// Rows sent to the database whose transaction has not finished, per
  /// account: reads include them with [_dbPending].
  final Map<MessageScope, List<List<MessageRow>>> _dbInFlight = {};

  /// Live range changes sent with a write that has not finished.
  final Map<MessageScope, List<Map<String, _RangeChange>>> _rangesInFlight = {};
  final Map<MessageScope, List<Set<String>>> _keepInFlight = {};

  /// Watermarks waiting for the next write, per account.
  Map<MessageScope, int> _watermarks = {};

  /// Live range changes waiting for the next write, per account and room: a
  /// time starts one (replacing an ended one), null ends it.
  final Map<MessageScope, Map<String, _RangeChange>> _rangeChanges = {};

  /// Per account: only these rooms keep their live range with the next
  /// write (applied before [_rangeChanges]).
  final Map<MessageScope, Set<String>> _keepRanges = {};

  /// Database writes run one at a time, so a failed write's range ends are
  /// part of the next write, before any later watermark.
  Future<void> _dbChain = Future.value();

  /// The reconnect time last recorded in each account's database.
  final Map<MessageScope, int> _liveSinceRecorded = {};

  Timer? _timer;
  int? _liveSince;

  MessageScope? _fedScope;
  Set<String> _fedRooms = const {};
  final List<void Function(RoomCatchUp)> _catchUpListeners = [];

  /// Records that the live feed (re)connected now.
  void markLive() => _liveSince = _clock();

  /// Records which rooms the channel list's live feed covers for [scope].
  /// Their reconnect recovery is its catch-up ([publishCatchUp]). A room the
  /// feed stops covering loses its live range: later watermarks no longer
  /// vouch for it.
  void setFedRooms(MessageScope? scope, Set<String> rooms) {
    if (scope != null && scope == _fedScope) {
      final dropped = _fedRooms.difference(rooms);
      if (dropped.isNotEmpty) _endLiveRanges(scope, dropped);
    }
    _fedScope = scope;
    _fedRooms = Set.unmodifiable(rooms);
  }

  /// Whether the channel list's live feed covers [channelId] in [scope].
  bool feeds(MessageScope? scope, String channelId) =>
      scope != null && scope == _fedScope && _fedRooms.contains(channelId);

  /// Registers [listener] for catch-up results; returns its remover.
  void Function() listenCatchUps(void Function(RoomCatchUp) listener) {
    _catchUpListeners.add(listener);
    return () => _catchUpListeners.remove(listener);
  }

  /// Tells open rooms what a reconnect catch-up fetched and left unfilled.
  void publishCatchUp(RoomCatchUp catchUp) {
    for (final listener in List.of(_catchUpListeners)) {
      listener(catchUp);
    }
  }

  /// Marks [rooms] as possibly missing rows: their live ranges end with the
  /// next database write (a hole [hasGap] reports at once).
  void markGap(MessageScope scope, Set<String> rooms) {
    if (rooms.isEmpty) return;
    _endLiveRanges(scope, rooms);
  }

  /// Whether [channelId] may be missing rows the live feed did not deliver:
  /// it has no open live range in the message database. False when there is
  /// no database to ask.
  Future<bool> hasGap(MessageScope scope, String channelId) async {
    final messages = _messages;
    if (messages == null) return false;
    await _dbChain;
    final key = scope;
    switch (_rangeChanges[key]?[channelId]) {
      case _RangeEnd():
        return true;
      case _RangeStart():
        return false;
      case null:
        if (_keepRanges[key] case final keep? when !keep.contains(channelId)) {
          return true;
        }
    }
    try {
      final liveFrom = await messages.read(key, LiveFromRead(channelId));
      if (liveFrom != null) return false;
      return await messages.isAvailable;
    } catch (error) {
      debugPrint('[ChannelRoomStore] live range of $channelId unknown: $error');
      return true;
    }
  }

  /// The room's newest rows were fetched: if the live feed covers it, its
  /// live range starts at [at] (by default now; an open one is kept). [at]
  /// may be the newest fetched row's time: the fetch proved nothing newer
  /// existed then, and the feed delivers everything after.
  void clearGap(MessageScope scope, String channelId, {int? at}) {
    if (_messages == null || !feeds(scope, channelId)) return;
    final from = at == null ? _clock() : min(at, _clock());
    final changes = _rangeChanges.putIfAbsent(scope, () => {});
    switch (changes[channelId]) {
      case _RangeStart():
        return;
      case _RangeEnd():
        changes[channelId] = _RangeStart(from, replace: true);
      case null:
        changes[channelId] = _RangeStart(from, replace: false);
    }
  }

  /// The newest stored page of [channelId] (or with [rootId], of that
  /// thread's replies), read synchronously, or null when nothing complete is
  /// stored (or there is no database to read).
  StoredTimelinePage? readNewest(
    MessageScope scope,
    String channelId, {
    String? rootId,
  }) => _read<StoredTimelinePage?>(
    scope,
    (db, liveFrom) => readNewestStoredPage(
      db,
      channelId,
      rootId: rootId,
      liveFrom: liveFrom(channelId),
    ),
  );

  /// The stored rows just older than [before], when they are known complete.
  StoredTimelinePage? readOlder(
    MessageScope scope,
    String channelId,
    CoverageCursor before, {
    String? rootId,
  }) => _read<StoredTimelinePage?>(
    scope,
    (db, liveFrom) => readOlderStoredPage(
      db,
      channelId,
      before,
      rootId: rootId,
      liveFrom: liveFrom(channelId),
    ),
  );

  /// The stored rows just newer than [after], oldest first, when they are
  /// known complete.
  StoredTimelinePage? readNewer(
    MessageScope scope,
    String channelId,
    CoverageCursor after, {
    String? rootId,
  }) => _read<StoredTimelinePage?>(
    scope,
    (db, liveFrom) => readNewerStoredPage(
      db,
      channelId,
      after,
      rootId: rootId,
      liveFrom: liveFrom(channelId),
    ),
  );

  /// What the stored main timeline proves about the first row after the
  /// read marker [readAt] that [counts] (see [readFirstStoredAfter]). Rows
  /// still queued for the database count too.
  StoredFirstAfter readFirstAfter(
    MessageScope scope,
    String channelId,
    int readAt,
    bool Function(NostrEvent row) counts,
  ) {
    final stored =
        _read<StoredFirstAfter>(
          scope,
          (db, liveFrom) => readFirstStoredAfter(
            db,
            channelId,
            readAt,
            counts,
            liveFrom: liveFrom(channelId),
          ),
        ) ??
        (known: false, row: null);
    NostrEvent? first = stored.row;
    for (final event in unflushed(scope, channelId)) {
      if (!isMainTimelineRow(event) ||
          event.createdAt <= readAt ||
          !counts(event)) {
        continue;
      }
      if (first == null || event.createdAt < first.createdAt) first = event;
    }
    // A queued row proves nothing about older rows the range lacks.
    return (known: stored.known, row: first);
  }

  /// When stored main-timeline row [id] was sent, or null when unknown.
  int? createdAtOf(MessageScope scope, String channelId, String id) {
    for (final event in unflushed(scope, channelId)) {
      if (event.id == id && isMainTimelineRow(event)) return event.createdAt;
    }
    return _read<int?>(
      scope,
      (db, _) => readStoredCreatedAt(db, channelId, id),
    );
  }

  /// When [channelId]'s open live range starts, counting range changes not
  /// yet written; null when it has none (or there is no database).
  int? liveFromOf(MessageScope scope, String channelId) =>
      _read<int?>(scope, (db, liveFrom) => liveFrom(channelId));

  /// Whether [channelId]'s newest message, sent at [latestAt], is stored with
  /// a complete range under it.
  bool isStoredThrough(MessageScope scope, String channelId, int latestAt) =>
      _read(
        scope,
        (db, liveFrom) => isStoredThroughIn(
          db,
          channelId,
          latestAt,
          liveFrom: liveFrom(channelId),
        ),
      ) ??
      false;

  /// Stored rows of [channelId] (or of its thread [rootId]) sent at or
  /// before [before], newest first, after the ones still queued.
  List<NostrEvent> readRowsBefore(
    MessageScope scope,
    String channelId, {
    required int before,
    String? rootId,
  }) => [
    ...unflushed(scope, channelId).where(
      (event) =>
          event.createdAt <= before &&
          EventKind.channelTimelineContentKinds.contains(event.kind) &&
          (rootId == null
              ? isMainTimelineRow(event)
              : event.id == rootId || event.threadReference.rootId == rootId),
    ),
    ...?_read(
      scope,
      (db, _) =>
          readStoredRowsBefore(db, channelId, before: before, rootId: rootId),
    ),
  ];

  /// Whether [channelId]'s live range is open: the live feed has delivered
  /// it continuously since that time. Counts range changes not yet written.
  bool hasOpenLiveRange(MessageScope scope, String channelId) =>
      liveFromOf(scope, channelId) != null;

  /// The events of [channelId] received but not yet in the database, in
  /// arrival order.
  List<NostrEvent> unflushed(MessageScope scope, String channelId) {
    final messages = _messages;
    if (messages == null) return const [];
    final key = scope;
    final pending = _dbPending[key];
    return [
      for (final batch in [
        ...?_dbInFlight[key],
        if (pending != null && !messages.isCleared(key, pending.generation))
          pending.rows,
      ])
        for (final row in batch)
          if (row.channelId == channelId) row.event,
    ];
  }

  /// Runs [query] on [scope]'s read connection. Its second argument gives a
  /// room's live range start, counting changes not yet written. Null when
  /// there is no database or the read fails (a skipped paint, not an error).
  T? _read<T>(
    MessageScope scope,
    T Function(Database db, int? Function(String channelId) liveFrom) query,
  ) {
    final messages = _messages;
    if (messages == null) return null;
    final key = scope;
    final db = messages.reader(key);
    if (db == null) return null;
    try {
      return query(db, (channelId) => _liveFromNow(key, channelId, db));
    } on SqliteException catch (error) {
      debugPrint('[ChannelRoomStore] stored rows unreadable: $error');
      return null;
    }
  }

  int? _liveFromNow(MessageScope key, String channelId, Database db) {
    // Newest changes first: queued, then those being written.
    for (final changes in [
      _rangeChanges[key],
      ...?_rangesInFlight[key]?.reversed,
    ]) {
      switch (changes?[channelId]) {
        case _RangeEnd():
          return null;
        case _RangeStart(:final at, :final replace):
          return replace ? at : readLiveFrom(db, channelId) ?? at;
        case null:
      }
    }
    for (final keep in [_keepRanges[key], ...?_keepInFlight[key]]) {
      if (keep != null && !keep.contains(channelId)) return null;
    }
    return readLiveFrom(db, channelId);
  }

  /// The watermark stored for [scope]'s physical server, or null when none
  /// (or no database) exists.
  Future<int?> loadWatermark(MessageScope scope) async {
    final messages = _messages;
    if (messages == null) return null;
    try {
      return await messages.read<int?>(scope, WatermarkRead(scope.relayUrl));
    } catch (error) {
      debugPrint('[ChannelRoomStore] catch-up watermark unavailable: $error');
      return null;
    }
  }

  /// Stores [at] as [scope]'s watermark with the next write, which also
  /// holds every event queued before this call. It schedules no write of its
  /// own: a stored watermark that lags only makes the next catch-up reach
  /// further back.
  void advanceWatermark(MessageScope scope, int at) {
    if (_messages == null) return;
    final key = scope;
    final current = _watermarks[key];
    if (current != null && current >= at) return;
    _watermarks[key] = at;
  }

  /// Nothing vouches for any stored live range of [scope] (no watermark is
  /// stored for its server): they all end with the next write.
  void resetLiveRanges(MessageScope scope) =>
      _keepOnlyLiveRanges(scope, const {});

  void _keepOnlyLiveRanges(MessageScope scope, Set<String> rooms) {
    if (_messages == null) return;
    final key = scope;
    final current = _keepRanges[key];
    _keepRanges[key] = current == null ? rooms : current.intersection(rooms);
    _rangeChanges[key]?.removeWhere((room, _) => !rooms.contains(room));
  }

  /// Writes the catch-up of [scope]'s [rooms] now: the events it queued, the
  /// end of each [gaps] room's live range (and of every room it was not
  /// for) and [watermark], in one transaction. False when that transaction
  /// failed; nothing of it was stored then.
  Future<bool> commitCatchUp(
    MessageScope scope, {
    required Set<String> rooms,
    required Set<String> gaps,
    required int watermark,
  }) async {
    _keepOnlyLiveRanges(scope, rooms);
    markGap(scope, gaps);
    advanceWatermark(scope, watermark);
    final results = await flush();
    return results[scope] ?? true;
  }

  void _endLiveRanges(MessageScope scope, Iterable<String> rooms) {
    if (_messages == null) return;
    final changes = _rangeChanges.putIfAbsent(scope, () => {});
    // No write of their own: they join the next one, which is the first
    // that could store a watermark past them.
    for (final room in rooms) {
      changes[room] = const _RangeEnd();
    }
  }

  /// Queues [event], delivered by the channel list's live feed, for the
  /// message database.
  void ingest(MessageScope scope, NostrEvent event) {
    if (event.channelId == null) return;
    if (_queueRows(scope, [event])) _scheduleFlush();
  }

  /// Queues [events] for the message database only, without coverage: live
  /// events of a room's own subscription, events loaded by id, and history
  /// from the legacy websocket path.
  void record(MessageScope scope, Iterable<NostrEvent> events) {
    if (_queueRows(scope, events)) _scheduleFlush();
  }

  /// Writes a fetched page's [events] and the coverage [claims] it proves in
  /// one transaction, without waiting for the batch. [channelId] names the
  /// room of events that carry no `h` tag.
  Future<void> recordPage(
    MessageScope scope,
    Iterable<NostrEvent> events, {
    required String channelId,
    List<CoverageClaim> claims = const [],
  }) async {
    final messages = _messages;
    if (messages == null) return;
    final rows = messageRowsFor(
      events,
      receivedAt: _clock(),
      channelId: channelId,
    );
    final key = scope;
    final inFlight = _dbInFlight.putIfAbsent(key, () => [])..add(rows);
    try {
      await messages.write(key, MessageDbWrite(rows: rows, claims: claims));
    } catch (error) {
      debugPrint(
        '[ChannelRoomStore] failed to store a page of $channelId: $error',
      );
    } finally {
      inFlight.remove(rows);
    }
  }

  /// Whether any row was queued.
  bool _queueRows(MessageScope scope, Iterable<NostrEvent> events) {
    final messages = _messages;
    if (messages == null) return false;
    final rows = messageRowsFor(events, receivedAt: _clock());
    if (rows.isEmpty) return false;
    final key = scope;
    final pending = _dbPending.putIfAbsent(
      key,
      () => (generation: messages.generation, rows: []),
    );
    pending.rows.addAll(rows);
    _dbPendingCount += rows.length;
    return true;
  }

  void _scheduleFlush() {
    if (_dbPendingCount >= maxPendingEvents) {
      unawaited(flush());
    } else {
      _timer ??= Timer(debounce, () => unawaited(flush()));
    }
  }

  /// Writes every queued event now. Returns, per account written, whether
  /// its database transaction committed.
  Future<Map<MessageScope, bool>> flush() async {
    _timer?.cancel();
    _timer = null;
    // Rows and watermarks are taken together: a watermark never reaches a
    // transaction without the events queued before it.
    final dbBatch = _dbPending;
    _dbPending = {};
    _dbPendingCount = 0;
    final watermarks = _watermarks;
    _watermarks = {};
    return _writeDatabase(dbBatch, watermarks);
  }

  /// One transaction per account, after the previous flush's transactions.
  Future<Map<MessageScope, bool>> _writeDatabase(
    Map<MessageScope, ({int generation, List<MessageRow> rows})> batch,
    Map<MessageScope, int> watermarks,
  ) {
    final messages = _messages;
    if (messages == null) return Future.value(const {});
    final generation = messages.generation;
    final result = _dbChain.then((_) async {
      // Range changes are taken only now, so a failure of the previous
      // flush is part of this write.
      final scopes = {
        ...batch.keys,
        ...watermarks.keys,
        ..._rangeChanges.keys,
        ..._keepRanges.keys,
      };
      final done = <MessageScope, bool>{};
      await Future.wait([
        for (final scope in scopes)
          _writeScope(
            scope,
            batch[scope]?.rows ?? const [],
            watermarks[scope],
            _rangeChanges.remove(scope) ?? const {},
            _keepRanges.remove(scope),
            batch[scope]?.generation ?? generation,
          ).then((ok) => done[scope] = ok),
      ]);
      return done;
    });
    _dbChain = result.then<void>((_) {}, onError: (Object _) {});
    return result;
  }

  Future<bool> _writeScope(
    MessageScope scope,
    List<MessageRow> rows,
    int? watermark,
    Map<String, _RangeChange> ranges,
    Set<String>? keep,
    int generation,
  ) async {
    final liveSince = _liveSince;
    final recordLive =
        liveSince != null && _liveSinceRecorded[scope] != liveSince;
    final inFlight = _dbInFlight.putIfAbsent(scope, () => [])..add(rows);
    final rangesInFlight = _rangesInFlight.putIfAbsent(scope, () => [])
      ..add(ranges);
    final keepInFlight = _keepInFlight.putIfAbsent(scope, () => []);
    if (keep != null) keepInFlight.add(keep);
    final write = MessageDbWrite(
      rows: rows,
      liveFrom: {
        for (final MapEntry(key: room, value: change) in ranges.entries)
          if (change is _RangeEnd)
            room: null
          else if (change case _RangeStart(:final at, replace: true))
            room: at,
      },
      liveStart: {
        for (final MapEntry(key: room, value: change) in ranges.entries)
          if (change case _RangeStart(:final at, replace: false)) room: at,
      },
      keepLiveRanges: keep,
      sync: watermark != null || recordLive
          ? SyncStateUpdate(
              origin: scope.relayUrl,
              watermark: watermark,
              liveSince: recordLive ? liveSince : null,
            )
          : null,
    );
    try {
      await _messages!.write(scope, write, generation: generation);
      if (recordLive) _liveSinceRecorded[scope] = liveSince;
      return true;
    } catch (error) {
      debugPrint(
        '[ChannelRoomStore] failed to store ${rows.length} events: $error',
      );
      // Nothing of it was stored. The rooms of its rows are no longer
      // complete: their live ranges end with the next write, which also
      // carries the watermark again. Ends not stored are kept; starts are
      // dropped (a later page fetch starts them again).
      if (keep != null) {
        final current = _keepRanges[scope];
        _keepRanges[scope] = current == null
            ? keep
            : current.intersection(keep);
      }
      final changes = _rangeChanges.putIfAbsent(scope, () => {});
      final lost = {
        for (final MapEntry(key: room, value: change) in ranges.entries)
          if (change is _RangeEnd) room,
      };
      for (final room in lost) {
        changes[room] = switch (changes[room]) {
          // Started again since: replace the range the lost end was for.
          _RangeStart(:final at) => _RangeStart(at, replace: true),
          _ => const _RangeEnd(),
        };
      }
      for (final row in rows) {
        changes[row.channelId] = const _RangeEnd();
      }
      if (watermark != null) {
        _watermarks[scope] = max(_watermarks[scope] ?? watermark, watermark);
      }
      _timer ??= Timer(debounce, () => unawaited(flush()));
      return false;
    } finally {
      inFlight.remove(rows);
      rangesInFlight.remove(ranges);
      if (keep != null) keepInFlight.remove(keep);
    }
  }

  static int _nowSeconds() => DateTime.now().millisecondsSinceEpoch ~/ 1000;
}

/// A pending change to a room's live range.
sealed class _RangeChange {
  const _RangeChange();
}

final class _RangeEnd extends _RangeChange {
  const _RangeEnd();
}

/// Starts a range at [at]: where none is open, or replacing the range when
/// it [replace]s an end queued before.
final class _RangeStart extends _RangeChange {
  const _RangeStart(this.at, {required this.replace});

  final int at;
  final bool replace;
}

/// What one reconnect catch-up of the channel list's rooms found.
@immutable
class RoomCatchUp {
  const RoomCatchUp({
    required this.scope,
    required this.rooms,
    required this.gaps,
    required this.events,
  });

  final MessageScope scope;

  /// Rooms the catch-up was for.
  final Set<String> rooms;

  /// Rooms it could not fill: rows sent while offline may be missing.
  final Set<String> gaps;

  /// The events it fetched, by room, oldest first.
  final Map<String, List<NostrEvent>> events;
}

final channelRoomStoreProvider = Provider<ChannelRoomStore>((ref) {
  final store = ChannelRoomStore(messages: ref.watch(messageStoreProvider));
  ref.listen(relaySessionProvider.select((session) => session.status), (
    _,
    status,
  ) {
    if (status == SessionStatus.connected) store.markLive();
  }, fireImmediately: true);
  ref.onDispose(() => unawaited(store.flush()));
  return store;
});
