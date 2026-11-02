/// The user stream: one live REQ for all of the account's rooms, with the
/// room set kept by the relay, which picks the mode per connection.
/// Rules: docs/message-sync.md#user-stream; cases:
/// test-fixtures/user-stream.json.
library;

import 'dart:async';
import 'dart:convert';
import 'dart:math';

import 'package:flutter/foundation.dart';

import 'room_bundle_coverage.dart';
import 'user_stream_catch_up.dart';
import 'user_stream_counters.dart';

export 'user_stream_catch_up.dart';
export 'user_stream_counters.dart';

part 'user_stream_live.dart';

/// The relay-signed snapshot part kind; never stored.
const userStreamKind = 24220;

/// The kinds the stream REQ asks for: the relay accepts exactly this set
/// (`frozenKinds`); any change bumps the snapshot's `v`.
const userStreamFrozenKinds = <int>[
  5, 7, 9, 9005, 39005, 40001, 40002, 40003, 40008, 40009, 40099, //
  45001, 45003, 48100, 48101, 48102, 48103,
];

/// The room-id placeholder the relay expands to the account's rooms.
const userStreamJoined = r'$joined';

/// The stream REQ's filter (`request`).
Map<String, dynamic> userStreamFilter() => {
  '#h': const [userStreamJoined],
  'kinds': userStreamFrozenKinds,
  'limit': 0,
};

/// Whether a REQ filter (JSON) is the stream's.
bool isUserStreamFilter(Object? filter) =>
    filter is Map &&
    filter['#h'] is List &&
    (filter['#h'] as List).contains(userStreamJoined);

const userStreamMaxSeq = 9007199254740991; // 2^53 - 1
const userStreamMaxEpochLength = 64;
const userStreamFullCloseWindowSecs = 600;
const userStreamFullClosesForBackoff = 2;
const userStreamBackoffSecs = 3600;
const userStreamBackgroundResetSecs = 3600;
const userStreamLaggedJitterSecs = 10;
const userStreamSpanExtendSecs = 60;
const userStreamBusyRetryDelaysSecs = [1, 2, 4, 8, 16, 30, 30];

/// How long a CLOSED that the relay follows with a cancel waits for the
/// socket to go before the client closes it.
const userStreamAwaitCloseGrace = Duration(seconds: 2);

/// The `auth-required` retry waits this long: mobile has no hook for an
/// AUTH OK after the first.
const userStreamAuthRetryDelay = Duration(seconds: 1);

/// The build switch, `--dart-define=KUZZ_USER_STREAM=off|follow`.
enum UserStreamBuild { off, follow }

UserStreamBuild parseUserStreamBuild(String value) => switch (value) {
  '' || 'follow' => UserStreamBuild.follow,
  'off' => UserStreamBuild.off,
  _ => throw ArgumentError.value(
    value,
    'KUZZ_USER_STREAM',
    'must be `off` or `follow` (or unset for follow)',
  ),
};

/// This build's `KUZZ_USER_STREAM`, checked while compiling like
/// `KUZZ_ENV` (default_community.dart).
class _UserStreamDefine {
  const _UserStreamDefine(this.value)
    : assert(
        value == '' || value == 'follow' || value == 'off',
        'KUZZ_USER_STREAM must be `off` or `follow` (or unset for follow).',
      );

  final String value;
}

const _userStreamDefine = _UserStreamDefine(
  String.fromEnvironment('KUZZ_USER_STREAM'),
);

/// Tests set this to run with the stream off (or back to `null`).
@visibleForTesting
UserStreamBuild? debugUserStreamBuild;

UserStreamBuild get userStreamBuild =>
    debugUserStreamBuild ?? parseUserStreamBuild(_userStreamDefine.value);

enum UserStreamMode { shadow, live, gated }

UserStreamMode? _parseMode(Object? value) => switch (value) {
  'shadow' => UserStreamMode.shadow,
  'live' => UserStreamMode.live,
  'gated' => UserStreamMode.gated,
  _ => null,
};

/// An accepted snapshot (`part1`).
@immutable
class UserStreamSnapshot {
  const UserStreamSnapshot({
    required this.mode,
    required this.epoch,
    required this.rooms,
    required this.createdAt,
    required this.relayPubkey,
  });

  final UserStreamMode mode;
  final String epoch;

  /// Room id to its accepted sequence.
  final Map<String, int> rooms;

  /// Part 1's `created_at` (`prevEose` of the next connection).
  final int createdAt;

  /// The key that signed part 1: the relay's.
  final String relayPubkey;

  bool get live => mode != UserStreamMode.shadow;
}

/// Assembles the snapshot parts of one stream REQ and runs every `part1`
/// check: the store layer and the stream logic share this one.
class UserStreamAssembler {
  UserStreamMode? _mode;
  String? _epoch;
  int _parts = 0;
  int _received = 0;
  int _createdAt = 0;
  String _relayPubkey = '';
  final Map<String, int> _rooms = {};

  /// Set once any check failed.
  bool refused = false;

  /// Non-snapshot frames ignored before part 1.
  int ignoredBeforePart1 = 0;

  bool get hasPart1 => _received > 0;

  /// The mode part 1 named, once it passed its checks.
  UserStreamMode? get mode => refused ? null : _mode;

  /// The epoch part 1 named, once it passed its checks.
  String? get epoch => refused ? null : _epoch;

  /// Feeds a frame received before EOSE. Returns false once refused.
  bool feed(Map<String, dynamic> event) {
    if (refused) return false;
    if (event['kind'] != userStreamKind) {
      if (!hasPart1) ignoredBeforePart1++;
      return true;
    }
    final content = event['content'];
    Object? json;
    try {
      json = content is String ? jsonDecode(content) : null;
    } on FormatException {
      json = null;
    }
    if (json is! Map || !_take(json, event)) refused = true;
    return !refused;
  }

  bool _take(Map json, Map<String, dynamic> event) {
    _received++;
    if (_received == 1) {
      final kinds = json['kinds'];
      final epoch = json['epoch'];
      if (json['v'] != 1 ||
          json['op'] != 'snapshot' ||
          json['part'] != 1 ||
          json['parts'] is! int ||
          (json['parts'] as int) < 1 ||
          epoch is! String ||
          epoch.isEmpty ||
          epoch.length > userStreamMaxEpochLength ||
          kinds is! List ||
          !setEquals(kinds.toSet(), userStreamFrozenKinds.toSet())) {
        return false;
      }
      _mode = _parseMode(json['mode']);
      if (_mode == null) return false;
      _epoch = epoch;
      _parts = json['parts'] as int;
      _createdAt = event['created_at'] is int ? event['created_at'] as int : 0;
      _relayPubkey = event['pubkey'] is String ? event['pubkey'] as String : '';
    } else {
      if (_received > _parts) return false;
      if (json.containsKey('part') && json['part'] != _received) return false;
    }
    final rooms = json['rooms'];
    if (rooms is! Map) return false;
    for (final MapEntry(:key, :value) in rooms.entries) {
      if (key is! String ||
          value is! int ||
          value < 0 ||
          value > userStreamMaxSeq ||
          _rooms.containsKey(key)) {
        return false;
      }
      _rooms[key] = value;
    }
    return true;
  }

  /// At EOSE: the snapshot, or `null` when it is refused (including fewer
  /// parts than announced).
  UserStreamSnapshot? finish() {
    if (refused || _received == 0 || _received != _parts) {
      refused = true;
      return null;
    }
    return UserStreamSnapshot(
      mode: _mode!,
      epoch: _epoch!,
      rooms: Map.unmodifiable(_rooms),
      createdAt: _createdAt,
      relayPubkey: _relayPubkey,
    );
  }
}

/// The `closedReasons` key of a CLOSED message on the stream sub.
String userStreamClosedKey(String reason) {
  if (reason.startsWith('auth-required:')) return 'auth_required';
  if (reason.startsWith('invalid: user-stream')) return 'invalid';
  return switch (reason) {
    'unsupported: user-stream off' => 'off',
    'unsupported: user-stream not enabled' => 'not_enabled',
    'unsupported: user-stream too many rooms' => 'too_many_rooms',
    'restricted: not a channel member' => 'restricted',
    'error: user-stream overflow' => 'overflow',
    'error: user-stream lagged' => 'lagged',
    'error: user-stream closed' => 'closed',
    _ => 'other',
  };
}

enum UserStreamClosedAction {
  chunks,
  fallback,
  retryAfterAuth,
  awaitSocketClose,
}

/// What a CLOSED on the stream sub means (`closed`).
@immutable
class UserStreamClosedVerdict {
  const UserStreamClosedVerdict({
    required this.action,
    required this.key,
    this.countsTowardBackoff = false,
    this.reconnectJitter = false,
    this.bugLog = false,
  });

  final UserStreamClosedAction action;

  /// The counter key: `overflow` splits into `shadow_full` and `full`.
  final String key;
  final bool countsTowardBackoff;
  final bool reconnectJitter;
  final bool bugLog;
}

/// [mode] is part 1's (null before it); [streamLive] whether the stream
/// EOSE was accepted; [afterRetry] whether the `auth-required` retry was
/// already used on this connection.
UserStreamClosedVerdict userStreamClosedVerdict(
  String reason, {
  required UserStreamMode? mode,
  required bool streamLive,
  bool afterRetry = false,
}) {
  final key = userStreamClosedKey(reason);
  final liveMode = mode == UserStreamMode.live || mode == UserStreamMode.gated;
  final afterEose = streamLive && liveMode;
  switch (key) {
    case 'auth_required':
      return UserStreamClosedVerdict(
        action: afterRetry
            ? UserStreamClosedAction.chunks
            : UserStreamClosedAction.retryAfterAuth,
        key: key,
      );
    case 'overflow':
      if (mode == UserStreamMode.shadow) {
        return const UserStreamClosedVerdict(
          action: UserStreamClosedAction.chunks,
          key: 'shadow_full',
        );
      }
      return const UserStreamClosedVerdict(
        action: UserStreamClosedAction.awaitSocketClose,
        key: 'full',
        countsTowardBackoff: true,
      );
    case 'lagged':
      return const UserStreamClosedVerdict(
        action: UserStreamClosedAction.awaitSocketClose,
        key: 'lagged',
        reconnectJitter: true,
      );
    case 'closed' || 'other':
      return UserStreamClosedVerdict(
        action: afterEose
            ? UserStreamClosedAction.fallback
            : UserStreamClosedAction.chunks,
        key: key,
        countsTowardBackoff: afterEose,
      );
    default:
      return UserStreamClosedVerdict(
        action: afterEose
            ? UserStreamClosedAction.fallback
            : UserStreamClosedAction.chunks,
        key: key,
        bugLog: key == 'invalid',
      );
  }
}

/// The overflow backoff of one account on one server (`backoff`), in unix
/// seconds. Kept in `sync_state` through [encode]/[decode].
class UserStreamBackoff {
  final List<int> _closes = [];
  int _blockedUntil = 0;

  bool allowed(int now) => now >= _blockedUntil;

  /// A counting close at [at].
  void noteClose(int at) {
    _closes
      ..add(at)
      ..removeWhere((t) => at - t > userStreamFullCloseWindowSecs);
    if (_closes.length >= userStreamFullClosesForBackoff) {
      _blockedUntil = at + userStreamBackoffSecs;
    }
  }

  /// A network change: ends the backoff and forgets earlier closes.
  void reset() {
    _closes.clear();
    _blockedUntil = 0;
  }

  /// The app came back after [backgroundSecs] in the background.
  void resumed(int backgroundSecs) {
    if (backgroundSecs >= userStreamBackgroundResetSecs) reset();
  }

  String encode() => jsonEncode({'closes': _closes, 'until': _blockedUntil});

  static UserStreamBackoff decode(String? text) {
    final backoff = UserStreamBackoff();
    if (text == null) return backoff;
    try {
      final json = jsonDecode(text);
      if (json is! Map) return backoff;
      if (json['closes'] case final List closes) {
        backoff._closes.addAll(closes.whereType<int>());
      }
      if (json['until'] case final int until) backoff._blockedUntil = until;
    } on FormatException {
      // A damaged value starts over.
    }
    return backoff;
  }
}

/// The stream's coverage span on one connection (`spanStop`): from its EOSE,
/// extended by stream frames at most every [userStreamSpanExtendSecs], and
/// never after its CLOSED.
class UserStreamSpan {
  UserStreamSpan(this.from) : to = from;

  final int from;
  int to;
  bool stopped = false;

  /// A stream frame received at [at]; true when the span grew.
  bool noteFrame(int at) {
    if (stopped || at - to < userStreamSpanExtendSecs) return false;
    to = at;
    return true;
  }

  void stop() => stopped = true;
}

enum _Verdict { pending, keep, skip }

/// What the stream is for the connection now.
enum UserStreamPhase {
  /// Not requested (build switch, backoff) or gone (CLOSED, refused).
  none,

  /// Requested; no accepted EOSE yet.
  pending,

  /// Accepted EOSE in shadow.
  shadow,

  /// Accepted EOSE in live or gated: the stream carries its rooms.
  live,
}

/// The stream state of one connection.
class _Connection {
  _Connection(this.generation, this.subId, this.sentAtMs);

  final int generation;
  final String subId;
  int sentAtMs;
  _Verdict verdict = _Verdict.pending;
  bool ended = false;
  bool authRetried = false;
  UserStreamAssembler assembler = UserStreamAssembler();
  UserStreamSnapshot? snapshot;
  UserStreamPhase phase = UserStreamPhase.pending;
  int? lastFrame;
  ShadowComparator? comparator;
  Timer? timer;

  /// Ids received live on this connection (stream or chunk).
  final liveIds = <String>{};

  /// What a `live`/`gated` EOSE carries; null otherwise or after a fallback.
  _Carry? carry;

  /// Shadow: rooms predicted as skips, until a chunk catch-up verifies them.
  final predicted = <String>{};

  /// The stream catch-up begun at the EOSE or the fallback.
  Future<void>? settled;
}

/// What the session gives the stream.
class UserStreamHost {
  const UserStreamHost({
    required this.nextSubId,
    required this.send,
    required this.isActive,
    required this.store,
    required this.closeSocket,
    required this.scope,
    required this.readState,
    required this.writeState,
    required this.chunkRooms,
    this.nowMs = _nowMs,
    this.timer = Timer.new,
    this.queueEpoch = _ignoreEpoch,
    this.capture = _noCapture,
    this.extendCoverage = _noCoverage,
    this.catchUp = _noCatchUp,
  });

  final String Function() nextSubId;
  final void Function(List<Object?> frame) send;

  /// Whether connection [generation] is still current and connected.
  final bool Function(int generation) isActive;

  /// Keeps a stream frame in the device store.
  final void Function(Map<String, dynamic> event) store;
  final void Function(int generation) closeSocket;

  /// The account and server the counters and backoff belong to; `null`
  /// with no store scope.
  final Object? Function() scope;
  final Future<Map<String, String>> Function(List<String> keys) readState;
  final void Function(Map<String, String> values) writeState;

  /// Rooms a live chunk carries now.
  final Set<String> Function() chunkRooms;
  final int Function() nowMs;
  final Timer Function(Duration, void Function()) timer;

  /// An accepted part 1's epoch (docs/message-sync.md#user-stream).
  final void Function(String epoch) queueEpoch;

  /// The store capture now (a span's `captured`), if any.
  final int? Function() capture;

  /// Records `[from, to]` for [rooms] in the store.
  final void Function(List<String> rooms, int from, int to, int? captured)
  extendCoverage;

  /// Runs a stream catch-up; `null` with no store.
  final Future<UserStreamCatchUpResult?> Function(
    UserStreamCatchUpJob job,
    bool Function(String id) receivedLive,
  )
  catchUp;

  static int _nowMs() => DateTime.now().millisecondsSinceEpoch;
  static void _ignoreEpoch(String _) {}
  static int? _noCapture() => null;
  static void _noCoverage(List<String> _, int _, int _, int? _) {}
  static Future<UserStreamCatchUpResult?> _noCatchUp(
    UserStreamCatchUpJob _,
    bool Function(String) _,
  ) async => null;
}

const _countersKey = 'user_stream_counters';
const _backoffKey = 'user_stream_backoff';
const _livePrevKey = 'user_stream_live_prev';

/// The stream of one relay session, across its connections.
class UserStream {
  UserStream(this.host);

  final UserStreamHost host;
  _Connection? _connection;
  final _random = Random();

  Object? _scope;
  Future<void>? _loading;
  bool _loaded = false;
  UserStreamCounters _totals = UserStreamCounters();
  UserStreamBackoff _backoff = UserStreamBackoff();
  final _reporter = UserStreamReporter();
  bool _jitterNext = false;
  bool _connectedBefore = false;

  final _listeners = <void Function()>[];
  final _frameListeners =
      <void Function(Map<String, dynamic> event, bool render)>[];

  /// The last accepted snapshot (rule W's order count).
  UserStreamSnapshot? _lastSnapshot;

  /// The previous live connection, for `liveMissed`.
  UserStreamLivePrev? _livePrev;

  /// The stream's say in chunk catch-ups.
  late final StoreStreamHooks hooks = _UserStreamHooks(this);

  UserStreamPhase get phase => _connection?.phase ?? UserStreamPhase.none;

  /// The accepted snapshot of the current connection, if any.
  UserStreamSnapshot? get snapshot => _connection?.snapshot;

  /// Cumulative counters (tests and the report).
  UserStreamCounters get totals => _totals;

  /// Called when [phase] changes.
  void Function() addListener(void Function() listener) {
    _listeners.add(listener);
    return () => _listeners.remove(listener);
  }

  void _notify() {
    for (final listener in [..._listeners]) {
      listener();
    }
  }

  /// AUTH OK on connection [generation]: the stream REQ goes first, unless
  /// the build switch or the backoff says chunks.
  Future<void> connected(int generation) async {
    _end();
    if (userStreamBuild == UserStreamBuild.off) return;
    final scope = host.scope();
    if (scope == null) return;
    if (scope != _scope || _loading == null) {
      _scope = scope;
      _loaded = false;
      final loading = _loading = _load();
      unawaited(loading.then((_) => _loaded = _loading == loading));
    }
    // Loaded: the REQ goes out now, ahead of everything else.
    if (!_loaded) await _loading;
    if (!host.isActive(generation) || _connection != null) return;
    if (!_backoff.allowed(host.nowMs() ~/ 1000)) return;
    if (_connectedBefore) _count((c) => c.add('reconnects'));
    _connectedBefore = true;
    final connection = _Connection(generation, host.nextSubId(), host.nowMs());
    _connection = connection;
    _sendReq(connection);
    _notify();
  }

  Future<void> _load() async {
    final values = await host.readState(const [
      _countersKey,
      _backoffKey,
      _livePrevKey,
    ]);
    _totals = UserStreamCounters.decode(values[_countersKey]);
    _backoff = UserStreamBackoff.decode(values[_backoffKey]);
    _livePrev = UserStreamLivePrev.decode(values[_livePrevKey]);
    _lastSnapshot = null;
    _reporter.loaded(_totals, host.nowMs());
  }

  void _sendReq(_Connection connection) {
    connection
      ..verdict = _Verdict.pending
      ..phase = UserStreamPhase.pending
      ..snapshot = null
      ..assembler = UserStreamAssembler()
      ..sentAtMs = host.nowMs();
    host.send(['REQ', connection.subId, userStreamFilter()]);
  }

  /// A client CLOSE of the stream sub: its frames are skipped from now on.
  @visibleForTesting
  void debugClientClose() {
    final connection = _connection;
    if (connection == null) return;
    connection
      ..verdict = _Verdict.skip
      ..phase = UserStreamPhase.none;
    host.send(['CLOSE', connection.subId]);
  }

  /// The REQ again on the same id: the verdict starts over at pending.
  @visibleForTesting
  void debugResendReq() {
    final connection = _connection;
    if (connection != null) _sendReq(connection);
  }

  /// The socket of connection [generation] ended.
  void socketEnded(int generation) {
    if (_connection?.generation == generation) _end();
  }

  void _end() {
    final connection = _connection;
    if (connection == null) return;
    _connection = null;
    connection
      ..ended = true
      ..timer?.cancel();
    _stopComparator(connection);
    if (connection.phase != UserStreamPhase.none) {
      connection.phase = UserStreamPhase.none;
      _notify();
    }
    _report(force: true);
  }

  /// The reconnect delay to add: 0–10 s after a `lagged` CLOSED, once.
  int takeReconnectJitterMs() {
    if (!_jitterNext) return 0;
    _jitterNext = false;
    return _random.nextInt(userStreamLaggedJitterSecs * 1000 + 1);
  }

  /// A network change ends the backoff.
  void networkChanged() {
    _backoff.reset();
    _saveBackoff();
  }

  /// The app resumed after [backgroundSecs] in the background.
  void resumed(int backgroundSecs) {
    if (backgroundSecs < userStreamBackgroundResetSecs) return;
    _backoff.resumed(backgroundSecs);
    _saveBackoff();
  }

  void _saveBackoff() {
    if (_scope != null && _scope == host.scope()) {
      host.writeState({_backoffKey: _backoff.encode()});
    }
  }

  /// A relay frame; true when it belongs to the stream sub, which nothing
  /// else may see. Other EVENTs feed the shadow comparison.
  bool onMessage(List<dynamic> data) {
    if (data.length < 2 || data[1] is! String) return false;
    final connection = _connection;
    final subId = data[1] as String;
    if (connection == null || subId != connection.subId) return false;
    switch (data[0]) {
      case 'EVENT' when data.length >= 3 && data[2] is Map<String, dynamic>:
        _noteLiveId(connection, (data[2] as Map)['id']);
        _onEvent(connection, data[2] as Map<String, dynamic>);
      case 'EOSE':
        _onEose(connection);
      case 'CLOSED':
        _onClosed(connection, data.length >= 3 ? '${data[2]}' : '');
    }
    return true;
  }

  /// An EVENT of a live chunk carrying rooms (shadow comparison).
  void noteChunkEvent(String id, String? room) {
    _connection?.comparator?.observe(
      onStream: false,
      id: id,
      room: room,
      atMs: host.nowMs(),
    );
  }

  void _onEvent(_Connection connection, Map<String, dynamic> event) {
    if (connection.verdict == _Verdict.skip &&
        connection.phase == UserStreamPhase.none) {
      return; // After a CLOSE or CLOSED: ignored.
    }
    final createdAt = event['created_at'] is int
        ? event['created_at'] as int
        : 0;
    final now = host.nowMs();
    final kind = event['kind'];
    if (connection.phase == UserStreamPhase.pending) {
      final before = connection.assembler.hasPart1;
      if (!connection.assembler.feed(event)) {
        _refuse(connection);
        return;
      }
      final epoch = connection.assembler.epoch;
      if (!before && epoch != null) host.queueEpoch(epoch);
      if (kind == userStreamKind || before) {
        connection.lastFrame = userStreamLastFrame(
          connection.lastFrame,
          createdAt,
          now ~/ 1000,
        );
      }
      return; // Pending: dropped.
    }
    if (kind == userStreamKind) return; // After EOSE: ignored.
    connection.lastFrame = userStreamLastFrame(
      connection.lastFrame,
      createdAt,
      now ~/ 1000,
    );
    if (connection.verdict == _Verdict.keep) {
      host.store(event);
      final render = _carryFrame(connection, event);
      for (final listener in [..._frameListeners]) {
        listener(event, render);
      }
    }
    connection.comparator?.observe(
      onStream: true,
      id: '${event['id']}',
      room: _roomOf(event),
      atMs: now,
    );
    if (connection.comparator != null) _takeComparisons(connection);
  }

  void _onEose(_Connection connection) {
    if (connection.phase != UserStreamPhase.pending) return;
    final snapshot = connection.assembler.finish();
    if (snapshot == null) {
      _refuse(connection);
      return;
    }
    final now = host.nowMs();
    connection
      ..snapshot = snapshot
      ..verdict = snapshot.live ? _Verdict.keep : _Verdict.skip
      ..phase = snapshot.live ? UserStreamPhase.live : UserStreamPhase.shadow;
    if (!snapshot.live) {
      connection.comparator = ShadowComparator(
        snapshotRooms: snapshot.rooms.keys.toSet(),
        startMs: now,
        chunkRooms: host.chunkRooms,
      );
    }
    _count((c) => c.snapshotMs = now - connection.sentAtMs);
    _startCatchUp(connection, snapshot);
    _notify();
  }

  /// A refused snapshot CLOSEs the stream: chunks for this connection.
  void _refuse(_Connection connection) {
    debugPrint('[user-stream] snapshot refused');
    connection
      ..verdict = _Verdict.skip
      ..phase = UserStreamPhase.none;
    host.send(['CLOSE', connection.subId]);
    _notify();
  }

  void _onClosed(_Connection connection, String reason) {
    if (connection.verdict == _Verdict.skip &&
        connection.phase == UserStreamPhase.none) {
      return;
    }
    final verdict = userStreamClosedVerdict(
      reason,
      mode: connection.assembler.mode ?? connection.snapshot?.mode,
      streamLive: connection.snapshot != null,
      afterRetry: connection.authRetried,
    );
    final generation = connection.generation;
    if (verdict.action == UserStreamClosedAction.retryAfterAuth) {
      connection
        ..authRetried = true
        ..verdict = _Verdict.pending
        ..timer?.cancel()
        ..timer = host.timer(userStreamAuthRetryDelay, () {
          if (_connection == connection && host.isActive(generation)) {
            _sendReq(connection);
          }
        });
      return;
    }
    if (verdict.bugLog) debugPrint('[user-stream] client bug: $reason');
    final wasLive = connection.phase == UserStreamPhase.live;
    connection
      ..verdict = _Verdict.skip
      ..phase = UserStreamPhase.none;
    _stopComparator(connection);
    connection.carry?.span.stop();
    if (verdict.reconnectJitter) _jitterNext = true;
    final at = host.nowMs() ~/ 1000;
    if (verdict.countsTowardBackoff) {
      _backoff.noteClose(at);
      _saveBackoff();
    }
    _count((c) => c.closed[verdict.key] = (c.closed[verdict.key] ?? 0) + 1);
    switch (verdict.action) {
      case UserStreamClosedAction.awaitSocketClose:
        connection.timer?.cancel();
        connection.timer = host.timer(userStreamAwaitCloseGrace, () {
          if (host.isActive(generation)) host.closeSocket(generation);
        });
      case UserStreamClosedAction.fallback when wasLive:
        _count(
          (c) => c.fallbacks[verdict.key] = (c.fallbacks[verdict.key] ?? 0) + 1,
        );
        fallback = (closedAt: at, snapshot: connection.snapshot!);
        _fallBack(connection, at);
      default:
        // Chunks were never closed: nothing is carried any more.
        connection.carry = null;
    }
    _notify();
  }

  /// The mid-connection fallback the last CLOSED asked for, until taken.
  ({int closedAt, UserStreamSnapshot snapshot})? fallback;

  void _stopComparator(_Connection connection) {
    final comparator = connection.comparator;
    if (comparator == null) return;
    comparator.stop(host.nowMs());
    _takeComparisons(connection);
    connection.comparator = null;
  }

  void _takeComparisons(_Connection connection) {
    final counts = connection.comparator?.takeCounts();
    if (counts == null || counts.isEmpty) return;
    _count((c) => c.addAll(counts));
  }

  void _count(void Function(UserStreamCounters counters) change) {
    change(_totals);
    _report();
  }

  void _report({bool force = false}) {
    if (_scope == null || _scope != host.scope()) return;
    final due = _reporter.due(_totals, host.nowMs());
    if (due) {
      _totals.seq++;
      debugPrint('[user-stream] report #${_totals.seq} ${_totals.encode()}');
    }
    if (due || force) host.writeState({_countersKey: _totals.encode()});
  }
}

String? _roomOf(Map<String, dynamic> event) {
  final tags = event['tags'];
  if (tags is! List) return null;
  for (final tag in tags) {
    if (tag is List && tag.length >= 2 && tag[0] == 'h' && tag[1] is String) {
      return tag[1] as String;
    }
  }
  return null;
}

/// The `h` room of an event (JSON).
String? userStreamRoomOf(Map<String, dynamic> event) => _roomOf(event);
