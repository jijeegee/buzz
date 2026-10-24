import 'dart:async';
import 'dart:convert';
import 'dart:math';

import 'package:flutter/foundation.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/message_db/coverage.dart';
import '../../shared/message_db/event_rows.dart';
import '../../shared/message_db/message_db.dart';
import '../../shared/message_db/message_store.dart';
import '../../shared/relay/relay.dart';
import 'channel_event_order.dart';
import 'channel_head_cache.dart';

/// Rows a room's stored tail keeps: one channel-window page.
const roomTailMaxRows = 50;

/// A room's newest top-level rows as this device knows them, kept on disk
/// next to the room's head (see [ChannelHeadCache]) and fed by live events for
/// every joined room, open or not.
///
/// Times are this device's clock in Unix seconds.
@immutable
class RoomTail {
  const RoomTail({
    this.rows = const [],
    this.aux = const [],
    this.contiguousFrom,
    this.syncedThrough,
    this.gap = false,
  });

  static const empty = RoomTail();

  /// Top-level timeline rows, oldest first, at most [roomTailMaxRows].
  final List<NostrEvent> rows;

  /// The newest edit and the newest thread summary of each row.
  final List<NostrEvent> aux;

  /// From this time on no top-level row is missing, apart from a [gap]. Zero
  /// when the rows reach the room's first message; null when unknown.
  final int? contiguousFrom;

  /// The tail is known complete up to this time, apart from a [gap].
  final int? syncedThrough;

  /// Rows may be missing between [contiguousFrom] and [syncedThrough]: the
  /// live feed was interrupted after [syncedThrough] and nothing has filled
  /// the hole since.
  final bool gap;

  /// The rows and their overlays, in the shape a channel window holds.
  List<NostrEvent> get events => [...rows, ...aux];

  /// The tail fields of a stored entry. Events whose id is in [storedIds]
  /// (the head stored beside it) are written as `{"ref": id}`.
  Map<String, Object?> toJson({Set<String> storedIds = const {}}) {
    Object encode(NostrEvent event) =>
        storedIds.contains(event.id) ? {'ref': event.id} : event.toJson();
    return {
      'tail': [for (final event in rows) encode(event)],
      'tail_aux': [for (final event in aux) encode(event)],
      'contiguous_from': contiguousFrom,
      'synced_through': syncedThrough,
      'gap': gap,
    };
  }

  /// Reads the tail fields of a stored entry, resolving `{"ref": id}` from
  /// [stored]; missing fields read as empty and unresolved refs are skipped.
  factory RoomTail.fromJson(
    Map<String, dynamic> json, {
    Map<String, Map<String, dynamic>> stored = const {},
  }) {
    List<NostrEvent> events(Object? list) {
      final result = <NostrEvent>[];
      for (final item in list is List ? list : const []) {
        final json = item as Map<String, dynamic>;
        final ref = json['ref'];
        final event = ref is String ? stored[ref] : json;
        if (event != null) result.add(NostrEvent.fromJson(event));
      }
      return result;
    }

    return RoomTail(
      rows: events(json['tail']),
      aux: events(json['tail_aux']),
      contiguousFrom: (json['contiguous_from'] as num?)?.toInt(),
      syncedThrough: (json['synced_through'] as num?)?.toInt(),
      gap: json['gap'] == true,
    );
  }
}

/// One live event as it arrived.
@immutable
class RoomLiveEvent {
  const RoomLiveEvent(
    this.event, {
    required this.liveSince,
    required this.receivedAt,
  });

  final NostrEvent event;

  /// When the live feed that delivered [event] last (re)connected: nothing
  /// sent before it is guaranteed to have arrived.
  final int liveSince;

  final int receivedAt;
}

/// Whether [event] is a row of a room's main timeline.
bool isRoomTailRow(NostrEvent event) =>
    EventKind.channelTimelineContentKinds.contains(event.kind) &&
    event.threadReference.parentId == null;

/// [tail] with [events] applied in arrival order: new rows join (newest
/// [maxRows] kept), edits and thread summaries overlay their row, deletions
/// remove it. A live feed that reconnected after [RoomTail.syncedThrough]
/// marks a [RoomTail.gap].
RoomTail applyLiveEvents(
  RoomTail tail,
  Iterable<RoomLiveEvent> events, {
  bool markGap = false,
  int maxRows = roomTailMaxRows,
}) {
  final builder = _TailBuilder(tail);
  if (markGap) builder.gap = true;
  for (final live in events) {
    final synced = builder.syncedThrough;
    if (synced == null) {
      // A first tail is complete from when its feed went live.
      if (builder.rows.isEmpty) builder.contiguousFrom ??= live.liveSince;
    } else if (synced < live.liveSince) {
      builder.gap = true;
    }
    builder.apply(live.event);
    builder.syncedThrough = max(synced ?? 0, live.receivedAt);
  }
  return builder.build(maxRows);
}

/// The tail after the relay's newest page [head] was fetched at [now]: the
/// page's rows plus rows of [existing] newer than all of them (live arrivals
/// that raced the fetch). The page is complete, so any gap closes.
RoomTail roomTailFromHead(
  List<NostrEvent> head,
  RoomTail existing, {
  required int now,
  int maxRows = roomTailMaxRows,
}) {
  final headRows = head.where(isRoomTailRow).toList()
    ..sort(compareChannelTimelineEventsChronologically);
  final builder = _TailBuilder(RoomTail.empty)..rows.addAll(headRows);
  final newest = headRows.lastOrNull;
  if (newest != null) {
    for (final row in existing.rows) {
      if (compareChannelTimelineEventsChronologically(row, newest) > 0) {
        builder.apply(row);
      }
    }
  }
  for (final event in head) {
    if (!isRoomTailRow(event)) builder.apply(event);
  }
  for (final event in existing.aux) {
    builder.apply(event);
  }
  builder
    ..contiguousFrom = _reachesFirstMessage(head)
        ? 0
        : headRows.firstOrNull?.createdAt
    ..syncedThrough = now
    ..gap = false;
  return builder.build(maxRows);
}

/// Whether the page's bounds say no older rows exist.
bool _reachesFirstMessage(List<NostrEvent> head) {
  for (final event in head) {
    if (event.kind != EventKind.channelWindowBounds) continue;
    try {
      final payload = jsonDecode(event.content);
      return payload is Map && payload['has_more'] == false;
    } catch (_) {
      return false;
    }
  }
  return false;
}

class _TailBuilder {
  _TailBuilder(RoomTail tail)
    : rows = [...tail.rows],
      aux = [...tail.aux],
      contiguousFrom = tail.contiguousFrom,
      syncedThrough = tail.syncedThrough,
      gap = tail.gap;

  final List<NostrEvent> rows;
  final List<NostrEvent> aux;
  int? contiguousFrom;
  int? syncedThrough;
  bool gap;

  void apply(NostrEvent event) {
    switch (event.kind) {
      case EventKind.deletion || EventKind.nip29DeleteEvent:
        final targets = {
          for (final tag in event.tags)
            if (tag.length >= 2 && tag[0] == 'e') tag[1],
        };
        rows.removeWhere((row) => targets.contains(row.id));
        aux.removeWhere(
          (overlay) =>
              targets.contains(overlay.id) ||
              targets.contains(_overlayTarget(overlay)),
        );
      case EventKind.streamMessageEdit || EventKind.channelThreadSummary:
        final target = _overlayTarget(event);
        if (target == null || !rows.any((row) => row.id == target)) return;
        final index = aux.indexWhere(
          (overlay) =>
              overlay.kind == event.kind && _overlayTarget(overlay) == target,
        );
        if (index == -1) {
          aux.add(event);
        } else if (event.createdAt >= aux[index].createdAt) {
          aux[index] = event;
        }
      default:
        if (isRoomTailRow(event) && !rows.any((row) => row.id == event.id)) {
          rows.add(event);
        }
    }
  }

  RoomTail build(int maxRows) {
    rows.sort(compareChannelTimelineEventsChronologically);
    if (rows.length > maxRows) {
      rows.removeRange(0, rows.length - maxRows);
      contiguousFrom = rows.first.createdAt;
    }
    final rowIds = {for (final row in rows) row.id};
    aux.removeWhere((overlay) => !rowIds.contains(_overlayTarget(overlay)));
    return RoomTail(
      rows: List.unmodifiable(rows),
      aux: List.unmodifiable(aux),
      contiguousFrom: contiguousFrom,
      syncedThrough: syncedThrough,
      gap: gap,
    );
  }
}

/// The row an edit (its last `e` tag, as desktop reads it) or a thread
/// summary (its `e` tag) belongs to.
String? _overlayTarget(NostrEvent event) {
  if (event.kind == EventKind.channelThreadSummary) {
    return event.getTagValue('e');
  }
  String? target;
  for (final tag in event.tags) {
    if (tag.length >= 2 && tag[0] == 'e') target = tag[1];
  }
  return target;
}

/// The one entry point for storing received events on this device. Live
/// events are collected per room and written in batches, so a busy feed costs
/// one disk write per room (stored tail) and one transaction per account
/// (message database) per [debounce]. Fetched pages go to the message
/// database at once, with the coverage they prove ([recordPage]).
///
/// It also keeps the channel list's catch-up state in the database: each
/// physical server's watermark, and each fed room's live range (see
/// coverage.dart). A watermark is written only in the transaction that
/// stores the events it vouches for, together with the end of every live
/// range those events could not keep complete.
class ChannelRoomStore {
  ChannelRoomStore(
    this._cache, {
    MessageStore? messages,
    this.debounce = const Duration(seconds: 2),
    int Function()? clock,
  }) : _messages = messages,
       _clock = clock ?? _nowSeconds;

  /// Pending events that force a write before [debounce] runs out.
  static const maxPendingEvents = 500;

  final ChannelHeadCache _cache;
  final MessageStore? _messages;
  final Duration debounce;
  final int Function() _clock;

  /// Rows waiting for the message database, per account, with the store
  /// generation they were queued under (a clear since drops them).
  Map<MessageScope, ({int generation, List<MessageRow> rows})> _dbPending = {};
  int _dbPendingCount = 0;

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

  Map<(ChannelHeadScope, String), List<RoomLiveEvent>> _pending = {};
  int _pendingCount = 0;
  int _pendingGeneration = 0;

  /// Rooms whose next tail write marks a gap: their last write failed, or a
  /// catch-up could not fill them.
  final Set<(ChannelHeadScope, String)> _tailGapRooms = {};
  Timer? _timer;
  int? _liveSince;

  ChannelHeadScope? _fedScope;
  Set<String> _fedRooms = const {};
  final List<void Function(RoomCatchUp)> _catchUpListeners = [];

  /// Records that the live feed (re)connected now.
  void markLive() => _liveSince = _clock();

  /// Records which rooms the channel list's live feed covers for [scope].
  /// Their reconnect recovery is its catch-up ([publishCatchUp]). A room the
  /// feed stops covering loses its live range: later watermarks no longer
  /// vouch for it.
  void setFedRooms(ChannelHeadScope? scope, Set<String> rooms) {
    if (scope != null && scope == _fedScope) {
      final dropped = _fedRooms.difference(rooms);
      if (dropped.isNotEmpty) _endLiveRanges(scope, dropped);
    }
    _fedScope = scope;
    _fedRooms = Set.unmodifiable(rooms);
  }

  /// Whether the channel list's live feed covers [channelId] in [scope].
  bool feeds(ChannelHeadScope? scope, String channelId) =>
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
  /// next database write (a hole [hasGap] reports at once), and each tail
  /// records the gap with its next write.
  void markGap(ChannelHeadScope scope, Set<String> rooms) {
    if (rooms.isEmpty) return;
    for (final room in rooms) {
      _tailGapRooms.add((scope, room));
    }
    _endLiveRanges(scope, rooms);
  }

  /// Whether [channelId] may be missing rows the live feed did not deliver:
  /// it has no open live range in the message database. False when there is
  /// no database to ask.
  Future<bool> hasGap(ChannelHeadScope scope, String channelId) async {
    final messages = _messages;
    if (messages == null) return false;
    await _dbChain;
    final key = _messageScope(scope);
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

  /// The room's newest page was fetched: if the live feed covers it, its
  /// live range starts now (an open one is kept).
  void clearGap(ChannelHeadScope scope, String channelId) {
    _tailGapRooms.remove((scope, channelId));
    if (_messages == null || !feeds(scope, channelId)) return;
    final changes = _rangeChanges.putIfAbsent(_messageScope(scope), () => {});
    switch (changes[channelId]) {
      case _RangeStart():
        return;
      case _RangeEnd():
        changes[channelId] = _RangeStart(_clock(), replace: true);
      case null:
        changes[channelId] = _RangeStart(_clock(), replace: false);
    }
  }

  /// The watermark stored for [scope]'s physical server, or null when none
  /// (or no database) exists.
  Future<int?> loadWatermark(ChannelHeadScope scope) async {
    final messages = _messages;
    if (messages == null) return null;
    try {
      return await messages.read<int?>(
        _messageScope(scope),
        WatermarkRead(scope.relayUrl),
      );
    } catch (error) {
      debugPrint('[ChannelRoomStore] catch-up watermark unavailable: $error');
      return null;
    }
  }

  /// Stores [at] as [scope]'s watermark with the next write, which also
  /// holds every event queued before this call. It schedules no write of its
  /// own: a stored watermark that lags only makes the next catch-up reach
  /// further back.
  void advanceWatermark(ChannelHeadScope scope, int at) {
    if (_messages == null) return;
    final key = _messageScope(scope);
    final current = _watermarks[key];
    if (current != null && current >= at) return;
    _watermarks[key] = at;
  }

  /// Nothing vouches for any stored live range of [scope] (no watermark is
  /// stored for its server): they all end with the next write.
  void resetLiveRanges(ChannelHeadScope scope) =>
      _keepOnlyLiveRanges(scope, const {});

  void _keepOnlyLiveRanges(ChannelHeadScope scope, Set<String> rooms) {
    if (_messages == null) return;
    final key = _messageScope(scope);
    final current = _keepRanges[key];
    _keepRanges[key] = current == null ? rooms : current.intersection(rooms);
    _rangeChanges[key]?.removeWhere((room, _) => !rooms.contains(room));
  }

  /// Writes the catch-up of [scope]'s [rooms] now: the events it queued, the
  /// end of each [gaps] room's live range (and of every room it was not
  /// for) and [watermark], in one transaction. False when that transaction
  /// failed; nothing of it was stored then.
  Future<bool> commitCatchUp(
    ChannelHeadScope scope, {
    required Set<String> rooms,
    required Set<String> gaps,
    required int watermark,
  }) async {
    _keepOnlyLiveRanges(scope, rooms);
    markGap(scope, gaps);
    advanceWatermark(scope, watermark);
    final results = await flush();
    return results[_messageScope(scope)] ?? true;
  }

  void _endLiveRanges(ChannelHeadScope scope, Iterable<String> rooms) {
    if (_messages == null) return;
    final changes = _rangeChanges.putIfAbsent(_messageScope(scope), () => {});
    // No write of their own: they join the next one, which is the first
    // that could store a watermark past them.
    for (final room in rooms) {
      changes[room] = const _RangeEnd();
    }
  }

  /// Queues [event] for the message database and its room's tail.
  /// [liveSince] is when the delivering feed became continuous (by default,
  /// now). Kinds the tail does not keep only go to the database.
  void ingest(ChannelHeadScope scope, NostrEvent event, {int? liveSince}) {
    final channelId = event.channelId;
    if (channelId == null) return;
    _queueRows(scope, [event]);
    if (!_storesKind(event.kind)) return;
    final generation = _cache.clearGeneration;
    if (_pending.isEmpty) {
      _pendingGeneration = generation;
    } else if (_pendingGeneration != generation) {
      // A clear since these were queued: they belong to removed data.
      _pending = {};
      _pendingCount = 0;
      _pendingGeneration = generation;
    }
    final now = _clock();
    _pending
        .putIfAbsent((scope, channelId), () => [])
        .add(
          RoomLiveEvent(event, liveSince: liveSince ?? now, receivedAt: now),
        );
    _pendingCount++;
    _scheduleFlush();
  }

  /// Queues [events] for the message database only, without coverage: live
  /// events of a room's own subscription, events loaded by id, and history
  /// from the legacy websocket path.
  void record(ChannelHeadScope scope, Iterable<NostrEvent> events) {
    if (_queueRows(scope, events)) _scheduleFlush();
  }

  /// Writes a fetched page's [events] and the coverage [claims] it proves in
  /// one transaction, without waiting for the batch. [channelId] names the
  /// room of events that carry no `h` tag.
  Future<void> recordPage(
    ChannelHeadScope scope,
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
    try {
      await messages.write(
        _messageScope(scope),
        MessageDbWrite(rows: rows, claims: claims),
      );
    } catch (error) {
      debugPrint(
        '[ChannelRoomStore] failed to store a page of $channelId: $error',
      );
    }
  }

  /// Whether any row was queued.
  bool _queueRows(ChannelHeadScope scope, Iterable<NostrEvent> events) {
    final messages = _messages;
    if (messages == null) return false;
    final rows = messageRowsFor(events, receivedAt: _clock());
    if (rows.isEmpty) return false;
    final key = _messageScope(scope);
    final pending = _dbPending.putIfAbsent(
      key,
      () => (generation: messages.generation, rows: []),
    );
    pending.rows.addAll(rows);
    _dbPendingCount += rows.length;
    return true;
  }

  void _scheduleFlush() {
    if (_pendingCount >= maxPendingEvents ||
        _dbPendingCount >= maxPendingEvents) {
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
    final batch = _pending;
    final generation = _pendingGeneration;
    _pending = {};
    _pendingCount = 0;
    final dbDone = _writeDatabase(dbBatch, watermarks);
    await Future.wait([
      for (final MapEntry(key: room, value: events) in batch.entries)
        _cache
            .applyLive(
              room.$1,
              room.$2,
              events,
              generation: generation,
              markGap: _tailGapRooms.remove(room),
            )
            .catchError((Object error) {
              // The events are lost to the tail; its next write says so.
              _tailGapRooms.add(room);
              debugPrint(
                '[ChannelRoomStore] failed to store live events for '
                '${room.$2}: $error',
              );
            }),
    ]);
    return dbDone;
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
    }
  }

  static MessageScope _messageScope(ChannelHeadScope scope) =>
      MessageScope(relayUrl: scope.relayUrl, pubkey: scope.pubkey);

  static bool _storesKind(int kind) =>
      EventKind.channelTimelineContentKinds.contains(kind) ||
      kind == EventKind.deletion ||
      kind == EventKind.nip29DeleteEvent ||
      kind == EventKind.streamMessageEdit ||
      kind == EventKind.channelThreadSummary;

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

  final ChannelHeadScope scope;

  /// Rooms the catch-up was for.
  final Set<String> rooms;

  /// Rooms it could not fill: rows sent while offline may be missing.
  final Set<String> gaps;

  /// The events it fetched, by room, oldest first.
  final Map<String, List<NostrEvent>> events;
}

final channelRoomStoreProvider = Provider<ChannelRoomStore>((ref) {
  final store = ChannelRoomStore(
    ref.watch(channelHeadCacheProvider),
    messages: ref.watch(messageStoreProvider),
  );
  ref.listen(relaySessionProvider.select((session) => session.status), (
    _,
    status,
  ) {
    if (status == SessionStatus.connected) store.markLive();
  }, fireImmediately: true);
  ref.onDispose(() => unawaited(store.flush()));
  return store;
});
