import 'dart:async';

import 'package:flutter/widgets.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/relay/relay.dart';
import '../channels/agent_activity/observer_models.dart';
import '../channels/agent_activity/observer_subscription.dart';
import 'chat_list.dart';

// Which chats have an agent working right now, for the Chats row badge.
// Ported from desktop: `activeAgentTurnsStore.ts` (observer turns, the main
// signal), `agentWorkingSignal.ts` (typing as the fallback) and
// `chatWorking.ts` (which row shows the work).

/// The harness sends `turn_liveness` about every 10 s.
const _livenessIntervalMs = 10000;

/// A turn with no activity this long is dropped (a host that died).
const _removeAfterMs = _livenessIntervalMs * 5 ~/ 2;

/// When all of an agent's turns go quiet this long at once, its frame stream
/// is likely down: stop dropping its turns for a while.
const _frameGapPauseMs = _livenessIntervalMs * 2;

/// After this long, a silent agent counts as gone.
const _prunePauseMaxMs = 3 * 60000;

/// Most turns kept per agent (the harness's upper bound).
const _maxTurnsPerAgent = 32;
const _maxTerminalTombstones = _maxTurnsPerAgent * 4;
const _pruneInterval = Duration(seconds: 5);
const _nullChannelKey = '\u0000null-channel';
final _threadRootPattern = RegExp(r'^[0-9a-f]{64}$');

/// One agent working in one thread (or the channel itself when
/// [threadRootId] is null), since [anchorAtMs] on this phone's clock.
@immutable
class WorkingTurn {
  const WorkingTurn({
    required this.agentPubkey,
    required this.threadRootId,
    required this.anchorAtMs,
  });

  final String agentPubkey;
  final String? threadRootId;
  final int anchorAtMs;
}

/// Agents working in one channel, from the earliest start ([anchorAtMs],
/// this phone's clock).
@immutable
class ChatWorkingSummary {
  const ChatWorkingSummary({
    required this.channelId,
    required this.anchorAtMs,
    required this.agentPubkeys,
    this.turns,
  });

  final String channelId;
  final int anchorAtMs;
  final List<String> agentPubkeys;

  /// Observer turns per (agent, thread); null for typing-only work.
  final List<WorkingTurn>? turns;

  int get agentCount => agentPubkeys.length;
}

class _Turn {
  _Turn({
    required this.channelId,
    required this.threadRootId,
    required this.startedAt,
    required this.lastActivityAt,
  });

  final String channelId;
  final String? threadRootId;
  final int startedAt;
  int lastActivityAt;
}

int? _parseMs(String timestamp) =>
    DateTime.tryParse(timestamp)?.millisecondsSinceEpoch;

String? _threadRootOf(Object? payload) {
  if (payload is! Map) return null;
  final root = payload['threadRootEventId'];
  return root is String && _threadRootPattern.hasMatch(root) ? root : null;
}

/// Active agent turns from observer frames, as desktop's
/// `activeAgentTurnsStore`: `turn_started` adds a turn, `turn_completed`,
/// `turn_error` and `agent_panic` end it, stream frames and `turn_liveness`
/// keep it alive (and bring back one dropped while still running). Frames
/// are taken once each, in order per (agent, channel). Times are this
/// phone's milliseconds, passed in so tests can drive the clock.
class ActiveAgentTurns {
  final _turns = <String, Map<String, _Turn>>{};
  final _offsets = <String, int>{};
  final _watermarks = <String, Map<String, ObserverFrame>>{};
  final _terminals = <String, Map<String, int>>{};
  final _parsed = Expando<int>();

  bool get isEmpty => _turns.isEmpty;

  int _frameMs(ObserverFrame frame) =>
      _parsed[frame] ??= _parseMs(frame.timestamp) ?? 0;

  int _compare(ObserverFrame a, ObserverFrame b) {
    final byTime = _frameMs(a).compareTo(_frameMs(b));
    return byTime != 0 ? byTime : a.seq.compareTo(b.seq);
  }

  bool _sampleOffset(String agent, String timestamp, int nowMs) {
    final at = _parseMs(timestamp);
    if (at == null) return false;
    final sample = nowMs - at;
    final prior = _offsets[agent];
    if (prior != null && sample >= prior) return false;
    _offsets[agent] = sample;
    return true;
  }

  void _start(
    String agent,
    String channelId,
    String turnId,
    String timestamp,
    String? threadRootId,
    int nowMs,
  ) {
    final turns = _turns.putIfAbsent(agent, () => {});
    if (turns.length >= _maxTurnsPerAgent && !turns.containsKey(turnId)) {
      String? oldest;
      var oldestAt = 1 << 62;
      for (final entry in turns.entries) {
        if (entry.value.startedAt < oldestAt) {
          oldestAt = entry.value.startedAt;
          oldest = entry.key;
        }
      }
      if (oldest != null) turns.remove(oldest);
    }
    turns[turnId] = _Turn(
      channelId: channelId,
      threadRootId: threadRootId,
      startedAt: _parseMs(timestamp) ?? nowMs,
      lastActivityAt: nowMs,
    );
  }

  bool _recordActivity(String agent, String? turnId, int nowMs) {
    if (turnId == null) return false;
    final turn = _turns[agent]?[turnId];
    if (turn == null) return false;
    turn.lastActivityAt = nowMs;
    return true;
  }

  bool _resurrect(String agent, ObserverFrame frame, int nowMs) {
    final turnId = frame.turnId;
    final channelId = frame.channelId;
    if (turnId == null || channelId == null) return false;
    final terminalAt = _terminals[agent]?[turnId];
    final frameAt = _parseMs(frame.timestamp);
    if (terminalAt != null && (frameAt == null || frameAt <= terminalAt)) {
      return false;
    }
    final startedAt = frame.startedAt;
    final startedAtMs = startedAt == null ? null : _parseMs(startedAt);
    final safeStart =
        frameAt != null && startedAtMs != null && startedAtMs <= frameAt
        ? startedAt!
        : frame.timestamp;
    _start(
      agent,
      channelId,
      turnId,
      safeStart,
      _threadRootOf(frame.payload),
      nowMs,
    );
    return true;
  }

  void _recordTerminal(String agent, String turnId, int? terminalAt) {
    if (terminalAt == null) return;
    final terminals = _terminals.putIfAbsent(agent, () => {});
    terminals[turnId] = terminalAt;
    if (terminals.length > _maxTerminalTombstones) {
      terminals.remove(terminals.keys.first);
    }
  }

  void _end(String agent, String? turnId, String? channelId, int? at) {
    if (turnId != null) _recordTerminal(agent, turnId, at);
    final turns = _turns[agent];
    if (turns == null) return;
    if (turnId != null) {
      turns.remove(turnId);
    } else if (channelId != null) {
      for (final entry in turns.entries) {
        if (entry.value.channelId == channelId) {
          turns.remove(entry.key);
          _recordTerminal(agent, entry.key, at);
          break;
        }
      }
    }
    if (turns.isEmpty) _turns.remove(agent);
  }

  /// Takes one frame of [agentPubkey]'s stream. True when the working
  /// chats (or their start times) changed.
  bool process(String agentPubkey, ObserverFrame frame, int nowMs) {
    final agent = agentPubkey.toLowerCase();
    final marks = _watermarks.putIfAbsent(agent, () => {});
    final channelKey = frame.channelId ?? _nullChannelKey;
    final last = marks[channelKey];
    if (last != null && _compare(frame, last) <= 0) return false;
    marks[channelKey] = frame;
    final offsetChanged = _sampleOffset(agent, frame.timestamp, nowMs);

    switch (frame.kind) {
      case 'turn_started':
        final channelId = frame.channelId;
        if (channelId != null) {
          _start(
            agent,
            channelId,
            frame.turnId ?? 'seq-${frame.seq}',
            frame.timestamp,
            _threadRootOf(frame.payload),
            nowMs,
          );
          return true;
        }
      case 'turn_completed':
      case 'turn_error':
      case 'agent_panic':
        _end(agent, frame.turnId, frame.channelId, _parseMs(frame.timestamp));
        return true;
      case 'acp_read':
      case 'acp_write':
      case 'turn_liveness':
        final refreshed = _recordActivity(agent, frame.turnId, nowMs);
        if (!refreshed && _resurrect(agent, frame, nowMs)) return true;
    }
    return offsetChanged;
  }

  /// Drops turns quiet for 25 s, except while all of one agent's turns are
  /// quiet together (its stream is down) for up to 3 minutes. True when any
  /// turn was dropped.
  bool prune(int nowMs) {
    var changed = false;
    for (final agent in [..._turns.keys]) {
      final turns = _turns[agent]!;
      var latest = 0;
      for (final turn in turns.values) {
        if (turn.lastActivityAt > latest) latest = turn.lastActivityAt;
      }
      final silentFor = nowMs - latest;
      if (latest > 0 &&
          silentFor > _frameGapPauseMs &&
          silentFor < _prunePauseMaxMs) {
        continue;
      }
      final before = turns.length;
      turns.removeWhere(
        (_, turn) => nowMs - turn.lastActivityAt > _removeAfterMs,
      );
      if (turns.length != before) changed = true;
      if (turns.isEmpty) _turns.remove(agent);
    }
    return changed;
  }

  /// Working channels, each from its earliest live turn.
  Map<String, ChatWorkingSummary> byChannel() {
    final anchors = <String, int>{};
    final agents = <String, Set<String>>{};
    final turnsByChannel = <String, Map<String, WorkingTurn>>{};
    for (final MapEntry(key: agent, value: turns) in _turns.entries) {
      final offset = _offsets[agent] ?? 0;
      for (final turn in turns.values) {
        final anchor = turn.startedAt + offset;
        final channel = turn.channelId;
        final prior = anchors[channel];
        if (prior == null || anchor < prior) anchors[channel] = anchor;
        (agents[channel] ??= {}).add(agent);
        final perTurn = turnsByChannel[channel] ??= {};
        final key = '$agent ${turn.threadRootId ?? ''}';
        final existing = perTurn[key];
        if (existing == null || anchor < existing.anchorAtMs) {
          perTurn[key] = WorkingTurn(
            agentPubkey: agent,
            threadRootId: turn.threadRootId,
            anchorAtMs: anchor,
          );
        }
      }
    }
    return {
      for (final channel in anchors.keys)
        channel: ChatWorkingSummary(
          channelId: channel,
          anchorAtMs: anchors[channel]!,
          agentPubkeys: agents[channel]!.toList()..sort(),
          turns: turnsByChannel[channel]!.values.toList()
            ..sort(
              (a, b) => a.agentPubkey != b.agentPubkey
                  ? a.agentPubkey.compareTo(b.agentPubkey)
                  : a.anchorAtMs.compareTo(b.anchorAtMs),
            ),
        ),
    };
  }
}

/// Work shown per row: [rooms] by channel id, [threads] by thread root id.
@immutable
class ChatWorking {
  const ChatWorking({required this.rooms, required this.threads});

  static const none = ChatWorking(rooms: {}, threads: {});

  final Map<String, ChatWorkingSummary> rooms;
  final Map<String, ChatWorkingSummary> threads;
}

ChatWorkingSummary _summarize(
  ChatWorkingSummary summary,
  List<WorkingTurn> turns, [
  List<String> extra = const [],
]) {
  final pubkeys = {...turns.map((turn) => turn.agentPubkey), ...extra};
  final anchor = turns.isEmpty
      ? summary.anchorAtMs
      : turns.map((turn) => turn.anchorAtMs).reduce((a, b) => a < b ? a : b);
  return ChatWorkingSummary(
    channelId: summary.channelId,
    anchorAtMs: anchor,
    agentPubkeys: pubkeys.toList(),
  );
}

/// Puts each agent's work on its row, as desktop's `splitChatWorking`: a
/// turn in a listed thread goes to that thread's row; everything else (the
/// channel itself, a DM, a thread not listed, typing only) to the room row,
/// so the same work never shows twice.
ChatWorking splitChatWorking(
  Map<String, ChatWorkingSummary> byChannel,
  Iterable<ChatListRow> rows,
) {
  if (byChannel.isEmpty) return ChatWorking.none;
  final listedThreads = {
    for (final row in rows)
      if (row.isThread) '${row.channelId}:${row.rootId ?? row.key}',
  };
  final rooms = <String, ChatWorkingSummary>{};
  final threads = <String, ChatWorkingSummary>{};
  for (final MapEntry(key: channelId, value: summary) in byChannel.entries) {
    final turns = summary.turns ?? const <WorkingTurn>[];
    if (turns.isEmpty) {
      rooms[channelId] = summary;
      continue;
    }
    final roomTurns = <WorkingTurn>[];
    final threadTurns = <String, List<WorkingTurn>>{};
    for (final turn in turns) {
      final root = turn.threadRootId;
      if (root != null && listedThreads.contains('$channelId:$root')) {
        (threadTurns[root] ??= []).add(turn);
      } else {
        roomTurns.add(turn);
      }
    }
    final withTurns = {for (final turn in turns) turn.agentPubkey};
    final typingOnly = [
      for (final pubkey in summary.agentPubkeys)
        if (!withTurns.contains(pubkey)) pubkey,
    ];
    if (roomTurns.isNotEmpty || typingOnly.isNotEmpty) {
      rooms[channelId] = _summarize(summary, roomTurns, typingOnly);
    }
    for (final MapEntry(key: root, value: list) in threadTurns.entries) {
      threads[root] = _summarize(summary, list);
    }
  }
  return ChatWorking(rooms: rooms, threads: threads);
}

/// "45s", "3m 12s", "1h 2m 3s" (desktop `formatElapsed`).
String formatWorkingElapsed(int ms) {
  final totalSeconds = ms < 0 ? 0 : ms ~/ 1000;
  if (totalSeconds < 60) return '${totalSeconds}s';
  final seconds = totalSeconds % 60;
  final totalMinutes = totalSeconds ~/ 60;
  if (totalMinutes < 60) return '${totalMinutes}m ${seconds}s';
  return '${totalMinutes ~/ 60}h ${totalMinutes % 60}m ${seconds}s';
}

/// The badge text: elapsed, with "(N)" when several agents work.
String chatWorkingLabel(ChatWorkingSummary summary, int nowMs) {
  final elapsed = formatWorkingElapsed(nowMs - summary.anchorAtMs);
  return summary.agentCount > 1 ? '$elapsed (${summary.agentCount})' : elapsed;
}

/// My agents' active turns by channel, from the observer stream the app
/// already keeps open (no new subscription).
class ActiveAgentTurnsNotifier
    extends Notifier<Map<String, ChatWorkingSummary>> {
  ActiveAgentTurns _tracker = ActiveAgentTurns();
  final _taken = Expando<bool>();
  Timer? _pruneTimer;

  @override
  Map<String, ChatWorkingSummary> build() {
    // A different account or server starts over.
    ref.watch(relayConfigProvider.select((c) => '${c.baseUrl}|${c.nsec}'));
    _tracker = ActiveAgentTurns();
    _pruneTimer?.cancel();
    _pruneTimer = null;
    ref.onDispose(() {
      _pruneTimer?.cancel();
      _pruneTimer = null;
    });
    ref.listen(observerRelayProvider, (_, next) {
      if (_take(next)) _publish();
    });
    ref.listen(appLifecycleProvider, (_, next) {
      if (next == AppLifecycleState.resumed) {
        if (_tracker.prune(DateTime.now().millisecondsSinceEpoch)) {
          _publish();
        } else {
          _schedulePrune();
        }
      } else if (next == AppLifecycleState.paused) {
        _pruneTimer?.cancel();
        _pruneTimer = null;
      }
    });
    _take(ref.read(observerRelayProvider));
    _schedulePrune();
    return _tracker.byChannel();
  }

  bool _take(ObserverRelayState relay) {
    final now = DateTime.now().millisecondsSinceEpoch;
    var changed = false;
    for (final MapEntry(key: agent, value: frames)
        in relay.framesByAgent.entries) {
      // New frames land at (or near) the tail: walk back until a long run of
      // frames already taken, then process the new ones oldest first.
      final fresh = <ObserverFrame>[];
      var takenRun = 0;
      for (var i = frames.length - 1; i >= 0; i--) {
        final frame = frames[i];
        if (_taken[frame] == true) {
          if (++takenRun >= _takenRunLimit) break;
          continue;
        }
        takenRun = 0;
        _taken[frame] = true;
        fresh.add(frame);
      }
      for (final frame in fresh.reversed) {
        if (_tracker.process(agent, frame, now)) changed = true;
      }
    }
    return changed;
  }

  void _publish() {
    state = _tracker.byChannel();
    _schedulePrune();
  }

  void _schedulePrune() {
    if (_tracker.isEmpty ||
        ref.read(appLifecycleProvider) == AppLifecycleState.paused) {
      _pruneTimer?.cancel();
      _pruneTimer = null;
      return;
    }
    _pruneTimer ??= Timer.periodic(_pruneInterval, (_) {
      if (_tracker.prune(DateTime.now().millisecondsSinceEpoch)) _publish();
    });
  }
}

final activeAgentTurnsProvider =
    NotifierProvider<ActiveAgentTurnsNotifier, Map<String, ChatWorkingSummary>>(
      ActiveAgentTurnsNotifier.new,
    );

/// Already-taken frames in a row that end the walk back for new ones.
const _takenRunLimit = 50;
