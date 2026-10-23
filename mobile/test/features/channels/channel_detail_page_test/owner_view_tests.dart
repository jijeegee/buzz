part of '../channel_detail_page_test.dart';

void ownerViewTests() {
  final me = 'a' * 64;
  final agent = 'b' * 64;
  final other = 'c' * 64;

  Channel agentDm() => Channel(
    id: _channelId,
    name: 'DM',
    channelType: 'dm',
    description: '',
    visibility: 'private',
    createdBy: me,
    createdAt: DateTime(2025),
    memberCount: 2,
    participantPubkeys: [me, agent],
    isMember: true,
  );

  Map<String, UserProfile> agentProfile(String owner) => {
    agent: UserProfile(pubkey: agent, displayName: 'Bo', ownerPubkey: owner),
  };

  testWidgets('DM with my agent shows the agent tabs', (tester) async {
    await tester.pumpWidget(
      _buildTestable(
        messages: const [],
        channel: agentDm(),
        users: agentProfile(me),
        huddleCurrentPubkey: me,
      ),
    );
    await tester.pumpAndSettle();

    expect(find.byKey(const ValueKey('agent-dm-tab-chat')), findsOneWidget);
    expect(
      find.byKey(const ValueKey('agent-dm-tab-conversations')),
      findsOneWidget,
    );
    expect(find.byType(ComposeBar), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('the conversations tab counts chats with a dot', (tester) async {
    final now = DateTime.now().millisecondsSinceEpoch ~/ 1000;
    ChatListRow row(String id, int at) => ChatListRow(
      isThread: false,
      key: id,
      at: at,
      atUs: at * 1000000,
      channelId: id,
      channelType: 'dm',
      channelName: 'DM',
      dmMembers: [agent, other],
      latest: NostrEvent(
        id: 'm-$id',
        pubkey: other,
        createdAt: at,
        kind: 9,
        tags: const [],
        content: 'hi',
      ),
    );
    await tester.pumpWidget(
      _buildTestable(
        messages: const [],
        channel: agentDm(),
        users: agentProfile(me),
        huddleCurrentPubkey: me,
        overrides: [
          agentChatListProvider(agent).overrideWith(
            (ref) async => [
              row('x1', now + 600),
              row('x2', now + 300),
              row('x3', now - 600),
            ],
          ),
        ],
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('Bo’s conversations 2'), findsOneWidget);
    expect(find.text('Chat with Bo'), findsOneWidget);
  });

  testWidgets('DM with someone else\'s agent has no agent tabs', (
    tester,
  ) async {
    await tester.pumpWidget(
      _buildTestable(
        messages: const [],
        channel: agentDm(),
        users: agentProfile(other),
        huddleCurrentPubkey: me,
      ),
    );
    await tester.pumpAndSettle();

    expect(find.byKey(const ValueKey('agent-dm-tab-chat')), findsNothing);
    expect(find.byType(ComposeBar), findsOneWidget);
  });

  testWidgets('owner view is read-only with the Ask and Remove bar', (
    tester,
  ) async {
    final chat = Channel(
      id: _channelId,
      name: 'Team',
      channelType: 'stream',
      description: '',
      visibility: 'private',
      createdBy: other,
      createdAt: DateTime(2025),
      memberCount: 2,
      participantPubkeys: [agent, other],
      groupChat: true,
    );
    await tester.pumpWidget(
      _buildTestable(
        messages: [_textMsg(id: 'm1', pubkey: agent, content: 'hi')],
        channel: chat,
        users: agentProfile(me),
        huddleCurrentPubkey: me,
        home: ChannelDetailPage(
          channel: chat,
          ownerView: OwnerView(
            agentPubkey: agent,
            agentName: 'Bo',
            dmChannelId: _otherChannelId,
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.byType(ComposeBar), findsNothing);
    expect(find.byKey(const ValueKey('owner-view-bar')), findsOneWidget);
    expect(find.byKey(const ValueKey('owner-view-ask')), findsOneWidget);
    expect(find.byKey(const ValueKey('owner-view-remove')), findsOneWidget);
    expect(find.text('Read only · Bo’s chat'), findsOneWidget);
    expect(find.byKey(const ValueKey('agent-dm-tab-chat')), findsNothing);
    // The agent's message sits on the right, as the reader's own.
    final row = tester.widget<ChatBubbleRow>(find.byType(ChatBubbleRow).first);
    expect(row.isOwn, isTrue);
    expect(tester.takeException(), isNull);
  });

  testWidgets('owner view thread is read-only with the bar', (tester) async {
    final root = _textMsg(id: 'root', pubkey: agent, content: 'hi');
    final head = formatTimeline([root]).single;
    await tester.pumpWidget(
      _buildTestable(
        messages: [root],
        users: agentProfile(me),
        huddleCurrentPubkey: me,
        threadReplies: const {'root': []},
        home: ThreadDetailPage(
          threadHead: head,
          allMessages: [head],
          channelId: _channelId,
          currentPubkey: me,
          isMember: true,
          isArchived: false,
          ownerView: OwnerView(
            agentPubkey: agent,
            agentName: 'Bo',
            dmChannelId: _otherChannelId,
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.byType(ComposeBar), findsNothing);
    expect(find.byKey(const ValueKey('owner-view-ask')), findsOneWidget);
    expect(find.byKey(const ValueKey('owner-view-remove')), findsOneWidget);
    expect(tester.takeException(), isNull);
  });
}
