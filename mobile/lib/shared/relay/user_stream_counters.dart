/// The user stream's counters: the shadow comparison, the per-account
/// totals persisted in the store's `sync_state` and the report line.
/// Rules: docs/message-sync.md#counters; cases:
/// test-fixtures/user-stream.json (`comparator`, `lastFrame`).
library;

import 'dart:convert';

/// Counters a report carries, in report order.
const userStreamCounterNames = [
  'eventsCompared',
  'chunkOnly',
  'streamOnlyUnexpected',
  'streamOnlyExpected',
  'predictedSkips',
  'verifiedSkips',
  'falseSkip',
  'liveRoomsChecked',
  'liveMissed',
  'skipAudited',
  'skipAuditNew',
  'skipAuditFailed',
  'seqWriteSkippedOrder',
  'reconnects',
];

/// A line is logged when a counter changed by this share since the last one.
const userStreamReportChangeRatio = 0.1;

/// ... or this long after the last line, if anything changed.
const userStreamReportIntervalMs = 10 * 60 * 1000;

/// How long a comparison window stays open, inclusive.
const userStreamCompareWindowMs = 5000;

/// Cumulative counters of one account on one server.
class UserStreamCounters {
  UserStreamCounters();

  final Map<String, int> values = {};

  /// Stream CLOSEDs by `closedReasons` key.
  final Map<String, int> closed = {};

  /// CLOSEDs that led to the mid-connection fallback, by key.
  final Map<String, int> fallbacks = {};

  /// The latest snapshot's REQ-to-EOSE time.
  int snapshotMs = 0;

  /// Report lines logged so far: a gap shows lost counts.
  int seq = 0;

  int operator [](String name) => values[name] ?? 0;

  void add(String name, [int by = 1]) {
    if (by == 0) return;
    values[name] = this[name] + by;
  }

  void addAll(UserStreamCounters other) {
    for (final MapEntry(:key, :value) in other.values.entries) {
      add(key, value);
    }
    for (final MapEntry(:key, :value) in other.closed.entries) {
      closed[key] = (closed[key] ?? 0) + value;
    }
    for (final MapEntry(:key, :value) in other.fallbacks.entries) {
      fallbacks[key] = (fallbacks[key] ?? 0) + value;
    }
    if (other.snapshotMs != 0) snapshotMs = other.snapshotMs;
  }

  bool get isEmpty =>
      values.values.every((v) => v == 0) &&
      closed.isEmpty &&
      fallbacks.isEmpty &&
      snapshotMs == 0;

  UserStreamCounters copy() => UserStreamCounters()
    ..addAll(this)
    ..seq = seq;

  Map<String, Object> toJson() => {
    for (final name in userStreamCounterNames)
      if (this[name] != 0) name: this[name],
    if (closed.isNotEmpty) 'closed': Map.of(closed),
    if (fallbacks.isNotEmpty) 'fallbacks': Map.of(fallbacks),
    if (snapshotMs != 0) 'snapshotMs': snapshotMs,
  };

  String encode() => jsonEncode({...toJson(), 'seq': seq});

  static UserStreamCounters decode(String? text) {
    final counters = UserStreamCounters();
    if (text == null) return counters;
    final Object? json;
    try {
      json = jsonDecode(text);
    } on FormatException {
      return counters;
    }
    if (json is! Map) return counters;
    for (final name in userStreamCounterNames) {
      if (json[name] case final int value) counters.values[name] = value;
    }
    void readMap(Object? from, Map<String, int> into) {
      if (from is! Map) return;
      for (final MapEntry(:key, :value) in from.entries) {
        if (key is String && value is int) into[key] = value;
      }
    }

    readMap(json['closed'], counters.closed);
    readMap(json['fallbacks'], counters.fallbacks);
    if (json['snapshotMs'] case final int ms) counters.snapshotMs = ms;
    if (json['seq'] case final int seq) counters.seq = seq;
    return counters;
  }

  /// Whether any counter moved by [userStreamReportChangeRatio] since [last]
  /// (any change from 0 counts).
  bool changedEnoughSince(UserStreamCounters last) {
    bool moved(int was, int now) => was == 0
        ? now != 0
        : (now - was).abs() >= was * userStreamReportChangeRatio;
    for (final name in userStreamCounterNames) {
      if (moved(last[name], this[name])) return true;
    }
    for (final key in {...closed.keys, ...last.closed.keys}) {
      if (moved(last.closed[key] ?? 0, closed[key] ?? 0)) return true;
    }
    for (final key in {...fallbacks.keys, ...last.fallbacks.keys}) {
      if (moved(last.fallbacks[key] ?? 0, fallbacks[key] ?? 0)) return true;
    }
    return false;
  }

  bool sameAs(UserStreamCounters other) =>
      jsonEncode(toJson()) == jsonEncode(other.toJson());
}

/// `prevLastFrame`: the newest `min(created_at, receive time)` over a
/// connection's stream frames, part 1 included (`lastFrame`).
int? userStreamLastFrame(int? current, int createdAt, int receivedAt) {
  final at = createdAt < receivedAt ? createdAt : receivedAt;
  return current == null || at > current ? at : current;
}

/// The shadow comparison (`comparator`): starts at the stream EOSE. Each
/// event id is classified once, [userStreamCompareWindowMs] after it was
/// first seen on either side.
class ShadowComparator {
  ShadowComparator({
    required Set<String> snapshotRooms,
    required int startMs,
    required Set<String> Function() chunkRooms,
  }) : _snapshotRooms = snapshotRooms,
       _startMs = startMs,
       _chunkRooms = chunkRooms;

  final Set<String> _snapshotRooms;
  final int _startMs;

  /// Rooms a live chunk carries now.
  final Set<String> Function() _chunkRooms;

  /// Rooms a 44100 added on this connection.
  final Set<String> joined = {};

  final counts = UserStreamCounters();
  final _open = <String, _Window>{};
  final _classified = <String>{};
  bool _stopped = false;

  bool _compared(String room) =>
      _chunkRooms().contains(room) &&
      (_snapshotRooms.contains(room) || joined.contains(room));

  /// An event [id] of [room] seen on the stream ([onStream]) or a chunk at
  /// [atMs].
  void observe({
    required bool onStream,
    required String id,
    required String? room,
    required int atMs,
  }) {
    if (_stopped || atMs < _startMs || room == null) return;
    tick(atMs);
    if (_classified.contains(id)) return;
    final window = _open[id];
    if (window == null) {
      _open[id] = _Window(atMs, room, onStream: onStream);
    } else if (onStream) {
      window.stream = true;
    } else {
      window.chunk = true;
    }
  }

  /// Classifies every window older than [userStreamCompareWindowMs] at
  /// [nowMs].
  void tick(int nowMs) {
    if (_stopped) return;
    final due = [
      for (final MapEntry(:key, :value) in _open.entries)
        if (nowMs > value.firstMs + userStreamCompareWindowMs) key,
    ];
    for (final id in due) {
      final window = _open.remove(id)!;
      _remember(id);
      final compared = _compared(window.room);
      if (window.stream && !window.chunk) {
        if (compared) counts.add('eventsCompared');
        final expected =
            _snapshotRooms.contains(window.room) ||
            joined.contains(window.room);
        counts.add(expected ? 'streamOnlyExpected' : 'streamOnlyUnexpected');
        continue;
      }
      if (!compared) continue;
      counts.add('eventsCompared');
      if (!window.stream) counts.add('chunkOnly');
    }
  }

  /// The stream CLOSEd or the connection ended at [nowMs]: classify what is
  /// due, drop open windows, and ignore everything after.
  void stop(int nowMs) {
    tick(nowMs);
    _open.clear();
    _stopped = true;
  }

  /// Takes the counts since the last call.
  UserStreamCounters takeCounts() {
    final taken = UserStreamCounters()..addAll(counts);
    counts.values.clear();
    return taken;
  }

  void _remember(String id) {
    _classified.add(id);
    if (_classified.length > 20000) _classified.remove(_classified.first);
  }
}

class _Window {
  _Window(this.firstMs, this.room, {required bool onStream})
    : stream = onStream,
      chunk = !onStream;

  final int firstMs;
  final String room;
  bool stream;
  bool chunk;
}

/// When to log a report line and persist: on a [userStreamReportChangeRatio]
/// change since the last line, or [userStreamReportIntervalMs] after it if
/// anything changed.
class UserStreamReporter {
  UserStreamCounters _lastLine = UserStreamCounters();
  int _lastLineAtMs = 0;

  /// Whether [now] (the totals) warrants a line at [nowMs]; marks it logged.
  bool due(UserStreamCounters now, int nowMs) {
    if (now.sameAs(_lastLine)) return false;
    final elapsed = nowMs - _lastLineAtMs;
    if (!now.changedEnoughSince(_lastLine) &&
        elapsed < userStreamReportIntervalMs) {
      return false;
    }
    _lastLine = now.copy();
    _lastLineAtMs = nowMs;
    return true;
  }

  /// Totals loaded from the store count as already reported.
  void loaded(UserStreamCounters totals, int nowMs) {
    _lastLine = totals.copy();
    _lastLineAtMs = nowMs;
  }
}
