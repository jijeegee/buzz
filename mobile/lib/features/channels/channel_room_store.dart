import 'dart:async';
import 'dart:convert';
import 'dart:math';

import 'package:flutter/foundation.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

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

/// Collects live events per room and writes them to each room's stored tail
/// in batches, so a busy feed costs one disk write per room per [debounce].
class ChannelRoomStore {
  ChannelRoomStore(
    this._cache, {
    this.debounce = const Duration(seconds: 2),
    int Function()? clock,
  }) : _clock = clock ?? _nowSeconds;

  /// Pending events that force a write before [debounce] runs out.
  static const maxPendingEvents = 500;

  final ChannelHeadCache _cache;
  final Duration debounce;
  final int Function() _clock;

  Map<(ChannelHeadScope, String), List<RoomLiveEvent>> _pending = {};
  int _pendingCount = 0;
  int _pendingGeneration = 0;

  /// Rooms whose next write marks a gap: their last write failed, or a
  /// catch-up could not fill them while their events were queued.
  final Set<(ChannelHeadScope, String)> _gapRooms = {};
  Timer? _timer;

  ChannelHeadScope? _fedScope;
  Set<String> _fedRooms = const {};
  final List<void Function(RoomCatchUp)> _catchUpListeners = [];

  /// Records which rooms the channel list's live feed covers for [scope].
  /// Their reconnect recovery is its catch-up ([publishCatchUp]).
  void setFedRooms(ChannelHeadScope? scope, Set<String> rooms) {
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

  /// Marks [rooms] as possibly missing rows: known at once through [hasGap],
  /// and written to each tail with its next live write.
  void markGap(ChannelHeadScope scope, Set<String> rooms) {
    for (final room in rooms) {
      _gapRooms.add((scope, room));
    }
  }

  /// Whether a gap marked for [channelId] is still waiting for its write.
  bool hasGap(ChannelHeadScope scope, String channelId) =>
      _gapRooms.contains((scope, channelId));

  /// Forgets a marked gap once the room's newest page was fetched.
  void clearGap(ChannelHeadScope scope, String channelId) =>
      _gapRooms.remove((scope, channelId));

  /// Queues [event] for its room's tail. [liveSince] is when the delivering
  /// feed became continuous (by default, now). Kinds the tail does not keep
  /// are ignored.
  void ingest(ChannelHeadScope scope, NostrEvent event, {int? liveSince}) {
    final channelId = event.channelId;
    if (channelId == null || !_storesKind(event.kind)) return;
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
    if (_pendingCount >= maxPendingEvents) {
      unawaited(flush());
    } else {
      _timer ??= Timer(debounce, () => unawaited(flush()));
    }
  }

  /// Writes every queued event now.
  Future<void> flush() async {
    _timer?.cancel();
    _timer = null;
    if (_pending.isEmpty) return;
    final batch = _pending;
    final generation = _pendingGeneration;
    _pending = {};
    _pendingCount = 0;
    await Future.wait([
      for (final MapEntry(key: room, value: events) in batch.entries)
        _cache
            .applyLive(
              room.$1,
              room.$2,
              events,
              generation: generation,
              markGap: _gapRooms.remove(room),
            )
            .catchError((Object error) {
              // The events are lost to the tail; its next write says so.
              _gapRooms.add(room);
              debugPrint(
                '[ChannelRoomStore] failed to store live events for '
                '${room.$2}: $error',
              );
            }),
    ]);
  }

  static bool _storesKind(int kind) =>
      EventKind.channelTimelineContentKinds.contains(kind) ||
      kind == EventKind.deletion ||
      kind == EventKind.nip29DeleteEvent ||
      kind == EventKind.streamMessageEdit ||
      kind == EventKind.channelThreadSummary;

  static int _nowSeconds() => DateTime.now().millisecondsSinceEpoch ~/ 1000;
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
  final store = ChannelRoomStore(ref.watch(channelHeadCacheProvider));
  ref.onDispose(() => unawaited(store.flush()));
  return store;
});
