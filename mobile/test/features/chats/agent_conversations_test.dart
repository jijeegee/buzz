import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:kuzz/features/agents/owner_view/owner_view.dart';
import 'package:kuzz/features/channels/channel_mutes/channel_mutes_provider.dart';
import 'package:kuzz/features/channels/channel_mutes/channel_mutes_storage.dart';
import 'package:kuzz/features/chats/chat_list.dart';
import 'package:kuzz/features/chats/chats_page.dart';
import 'package:kuzz/shared/mentions/agent_identity_provider.dart';
import 'package:kuzz/shared/relay/relay.dart';
import 'package:kuzz/shared/theme/theme.dart';
import 'package:shared_preferences/shared_preferences.dart';

const _dmPerson = '11111111-1111-4111-8111-111111111111';
const _dmAgent = '22222222-2222-4222-8222-222222222222';
const _group = '33333333-3333-4333-8333-333333333333';
final _agent = 'b' * 64;
final _bob = 'c' * 64;
final _bot = 'd' * 64;

ChatListRow _row(
  String channelId,
  int at, {
  String type = 'dm',
  List<String> members = const [],
  bool groupChat = false,
  int? memberCount,
}) => ChatListRow(
  isThread: false,
  key: channelId,
  at: at,
  atUs: at * 1000000,
  channelId: channelId,
  channelType: type,
  channelName: type == 'dm' ? 'DM' : 'Team',
  dmMembers: members,
  groupChat: groupChat,
  memberCount: memberCount,
  latest: NostrEvent(
    id: 'm-$channelId',
    pubkey: _agent,
    createdAt: at,
    kind: 9,
    tags: const [],
    content: 'hi',
  ),
);

class _NoMutes extends ChannelMutesNotifier {
  @override
  ChannelMutesState build() =>
      const ChannelMutesState(isReady: true, store: ChannelMuteStore());
}

Future<void> _pump(WidgetTester tester, List<ChatListRow> rows) async {
  SharedPreferences.setMockInitialValues({});
  final prefs = await SharedPreferences.getInstance();
  await tester.pumpWidget(
    ProviderScope(
      overrides: [
        savedPrefsProvider.overrideWithValue(prefs),
        channelMutesProvider.overrideWith(_NoMutes.new),
        knownAgentPubkeysProvider.overrideWithValue({_bot}),
        agentChatListProvider(_agent).overrideWith((ref) async => rows),
      ],
      child: MaterialApp(
        theme: AppTheme.light(),
        home: Scaffold(
          body: AgentConversationsList(
            agentPubkey: _agent,
            agentName: 'Bo',
            dmChannelId: 'dm',
            topInset: 0,
          ),
        ),
      ),
    ),
  );
  await tester.pumpAndSettle();
}

String? _tag(WidgetTester tester, String key) {
  final finder = find.byKey(ValueKey('chat-row-tag-$key'));
  if (finder.evaluate().isEmpty) return null;
  return tester
      .widget<Text>(find.descendant(of: finder, matching: find.byType(Text)))
      .data;
}

void main() {
  testWidgets('rows are tagged Person, Agent or people, with no counts', (
    tester,
  ) async {
    final now = DateTime.now().millisecondsSinceEpoch ~/ 1000;
    await _pump(tester, [
      _row(_dmPerson, now, members: [_agent, _bob]),
      _row(_dmAgent, now - 60, members: [_bot, _agent]),
      _row(
        _group,
        now - 120,
        type: 'stream',
        members: [_bob],
        groupChat: true,
        memberCount: 3,
      ),
    ]);

    expect(find.byKey(const ValueKey('agent-chats-view-all')), findsOneWidget);
    expect(_tag(tester, _dmPerson), 'Person');
    expect(_tag(tester, _dmAgent), 'Agent');
    expect(_tag(tester, _group), '3 people');
    expect(find.byKey(ValueKey('chat-row-unread-$_dmPerson')), findsNothing);
    expect(tester.takeException(), isNull);
  });

  testWidgets('no chats shows the empty line', (tester) async {
    await _pump(tester, const []);
    expect(find.text('Bo has no other conversations yet'), findsOneWidget);
  });
}
