import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:kuzz/features/channels/agent_activity/observer_models.dart';
import 'package:kuzz/features/chats/chat_list.dart';
import 'package:kuzz/features/chats/chat_working.dart';
import 'package:kuzz/features/chats/chats_page.dart';
import 'package:kuzz/shared/theme/theme.dart';
import 'package:shared_preferences/shared_preferences.dart';

const _channel = '11111111-1111-4111-8111-111111111111';
const _other = '22222222-2222-4222-8222-222222222222';
final _agent = 'a' * 64;
final _agent2 = 'b' * 64;
final _root = 'c' * 64;
final _base = DateTime.utc(2026, 10, 18, 12).millisecondsSinceEpoch;

String _ts(int ms) =>
    DateTime.fromMillisecondsSinceEpoch(ms, isUtc: true).toIso8601String();

ObserverFrame _frame(
  int seq,
  int atMs,
  String kind, {
  String? channelId = _channel,
  String? turnId = 't1',
  String? root,
  String? startedAt,
}) => ObserverFrame(
  seq: seq,
  timestamp: _ts(atMs),
  kind: kind,
  channelId: channelId,
  turnId: turnId,
  startedAt: startedAt,
  payload: root == null ? null : {'threadRootEventId': root},
);

ChatListRow _threadRow(String channelId, String root) => ChatListRow(
  isThread: true,
  key: root,
  at: 1,
  atUs: 1000000,
  channelId: channelId,
  channelType: 'stream',
  channelName: 'general',
  rootId: root,
);

ChatListRow _roomRow(String channelId, {int at = 1}) => ChatListRow(
  isThread: false,
  key: channelId,
  at: at,
  atUs: at * 1000000,
  channelId: channelId,
  channelType: 'stream',
  channelName: 'general',
);

class _FixedTurns extends ActiveAgentTurnsNotifier {
  _FixedTurns(this.turns);

  final Map<String, ChatWorkingSummary> turns;

  @override
  Map<String, ChatWorkingSummary> build() => turns;
}

void main() {
  group('ActiveAgentTurns (desktop activeAgentTurnsStore)', () {
    test('a started turn works until it completes', () {
      final turns = ActiveAgentTurns();
      expect(
        turns.process(_agent, _frame(1, _base, 'turn_started'), _base + 500),
        isTrue,
      );
      final summary = turns.byChannel()[_channel]!;
      expect(summary.agentPubkeys, [_agent]);
      // Anchored to the agent's start, on this phone's clock (offset 500).
      expect(summary.anchorAtMs, _base + 500);

      expect(
        turns.process(
          _agent,
          _frame(2, _base + 3000, 'turn_completed'),
          _base + 3500,
        ),
        isTrue,
      );
      expect(turns.byChannel(), isEmpty);
    });

    test('a replayed or older frame is taken only once', () {
      final turns = ActiveAgentTurns();
      turns.process(_agent, _frame(1, _base, 'turn_started'), _base);
      turns.process(_agent, _frame(2, _base + 10, 'turn_completed'), _base);
      // The same start again (a replay) does not bring the turn back.
      expect(
        turns.process(_agent, _frame(1, _base, 'turn_started'), _base),
        isFalse,
      );
      expect(turns.byChannel(), isEmpty);
    });

    test('a quiet turn is dropped after 25s; liveness keeps it', () {
      final turns = ActiveAgentTurns()
        ..process(_agent, _frame(1, _base, 'turn_started'), _base)
        ..process(
          _agent,
          _frame(2, _base, 'turn_started', turnId: 't2'),
          _base,
        );
      // t2 stays fresh, so the agent's stream is up and quiet t1 goes.
      turns.process(
        _agent,
        _frame(3, _base + 20000, 'turn_liveness', turnId: 't2'),
        _base + 20000,
      );
      expect(turns.prune(_base + 26000), isTrue);
      final summary = turns.byChannel()[_channel]!;
      expect(summary.turns, hasLength(1));
    });

    test('all of an agent\'s turns quiet together: kept up to 3 minutes', () {
      final turns = ActiveAgentTurns()
        ..process(_agent, _frame(1, _base, 'turn_started'), _base);
      expect(turns.prune(_base + 60000), isFalse);
      expect(turns.byChannel(), isNotEmpty);
      expect(turns.prune(_base + 181000), isTrue);
      expect(turns.byChannel(), isEmpty);
    });

    test('liveness brings back a dropped turn, not a completed one', () {
      final turns = ActiveAgentTurns()
        ..process(_agent, _frame(1, _base, 'turn_started'), _base);
      turns.prune(_base + 200000);
      expect(turns.byChannel(), isEmpty);
      expect(
        turns.process(
          _agent,
          _frame(2, _base + 200000, 'turn_liveness', startedAt: _ts(_base)),
          _base + 200000,
        ),
        isTrue,
      );
      // The original start is kept, so the elapsed time does not reset.
      expect(turns.byChannel()[_channel]!.anchorAtMs, _base);

      turns.process(
        _agent,
        _frame(3, _base + 200100, 'turn_completed'),
        _base + 200100,
      );
      expect(
        turns.process(
          _agent,
          _frame(4, _base + 200050, 'turn_liveness', channelId: _other),
          _base + 200200,
        ),
        isFalse,
      );
      expect(turns.byChannel(), isEmpty);
    });

    test('the thread root comes from the payload', () {
      final turns = ActiveAgentTurns()
        ..process(_agent, _frame(1, _base, 'turn_started', root: _root), _base)
        ..process(
          _agent2,
          _frame(1, _base + 5, 'turn_started', root: 'not-a-root'),
          _base + 5,
        );
      final summary = turns.byChannel()[_channel]!;
      expect(summary.agentCount, 2);
      expect(summary.turns!.map((turn) => turn.threadRootId), [_root, null]);
    });
  });

  group('splitChatWorking (desktop chatWorking)', () {
    final summary = ChatWorkingSummary(
      channelId: _channel,
      anchorAtMs: _base,
      agentPubkeys: [_agent, _agent2],
      turns: [
        WorkingTurn(
          agentPubkey: _agent,
          threadRootId: _root,
          anchorAtMs: _base,
        ),
        WorkingTurn(
          agentPubkey: _agent2,
          threadRootId: null,
          anchorAtMs: _base + 1000,
        ),
      ],
    );

    test('a listed thread gets its own work; the rest stays on the room', () {
      final working = splitChatWorking(
        {_channel: summary},
        [_roomRow(_channel), _threadRow(_channel, _root)],
      );
      expect(working.threads[_root]!.agentPubkeys, [_agent]);
      expect(working.threads[_root]!.anchorAtMs, _base);
      expect(working.rooms[_channel]!.agentPubkeys, [_agent2]);
      expect(working.rooms[_channel]!.anchorAtMs, _base + 1000);
    });

    test('a thread not listed shows on the room row', () {
      final working = splitChatWorking(
        {_channel: summary},
        [_roomRow(_channel)],
      );
      expect(working.threads, isEmpty);
      expect(working.rooms[_channel]!.agentCount, 2);
    });
  });

  test('elapsed reads as desktop', () {
    expect(formatWorkingElapsed(45000), '45s');
    expect(formatWorkingElapsed(192000), '3m 12s');
    expect(formatWorkingElapsed(3723000), '1h 2m 3s');
    expect(formatWorkingElapsed(-5), '0s');
    final summary = ChatWorkingSummary(
      channelId: _channel,
      anchorAtMs: 0,
      agentPubkeys: [_agent, _agent2],
    );
    expect(chatWorkingLabel(summary, 65000), '1m 5s (2)');
  });

  testWidgets('the working badge sits left of the time and ticks', (
    tester,
  ) async {
    SharedPreferences.setMockInitialValues({});
    final prefs = await SharedPreferences.getInstance();
    final now = DateTime.now().millisecondsSinceEpoch;
    await tester.pumpWidget(
      ProviderScope(
        overrides: [
          savedPrefsProvider.overrideWithValue(prefs),
          chatsRowsProvider.overrideWithValue([
            _roomRow(_channel, at: now ~/ 1000),
            _roomRow(_other, at: now ~/ 1000 - 10),
          ]),
          activeAgentTurnsProvider.overrideWith(
            () => _FixedTurns({
              _channel: ChatWorkingSummary(
                channelId: _channel,
                anchorAtMs: now - 5000,
                agentPubkeys: [_agent],
                turns: [
                  WorkingTurn(
                    agentPubkey: _agent,
                    threadRootId: null,
                    anchorAtMs: now - 5000,
                  ),
                ],
              ),
            }),
          ),
        ],
        child: MaterialApp(theme: AppTheme.light(), home: const ChatsPage()),
      ),
    );
    await tester.pump();

    final badge = find.byKey(ValueKey('chat-row-working-$_channel'));
    expect(badge, findsOneWidget);
    expect(find.byKey(ValueKey('chat-row-working-$_other')), findsNothing);
    final time = tester.getRect(
      find.byKey(ValueKey('chat-row-time-$_channel')),
    );
    expect(tester.getRect(badge).right, lessThanOrEqualTo(time.left));
    expect(tester.getCenter(badge).dy, closeTo(time.center.dy, 1));
    // The time keeps its right edge with or without a badge.
    final otherTime = tester.getRect(
      find.byKey(ValueKey('chat-row-time-$_other')),
    );
    expect(time.right, closeTo(otherTime.right, 0.01));
    expect(
      find.descendant(of: badge, matching: find.textContaining('s')),
      findsOneWidget,
    );
  });
}
