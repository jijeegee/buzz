import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:kuzz/features/channels/channel.dart';
import 'package:kuzz/features/channels/channel_actions_sheet.dart';
import 'package:kuzz/features/channels/channel_management_provider.dart';
import 'package:kuzz/features/channels/channel_names/channel_names_provider.dart';
import 'package:kuzz/features/channels/channel_names/channel_names_storage.dart';
import 'package:kuzz/features/channels/group_chat/chat_info_sheet.dart';
import 'package:kuzz/shared/mentions/agent_identity_provider.dart';
import 'package:kuzz/shared/relay/relay.dart';
import 'package:kuzz/shared/theme/theme.dart';

final _me = 'a' * 64;
final _ann = 'b' * 64;
final _bob = 'c' * 64;
final _myAgent = 'd' * 64;
final _bobsAgent = 'e' * 64;

class _RecordingActions extends ChannelActions {
  _RecordingActions(Ref ref)
    : super(
        ref: ref,
        session: ref.read(relaySessionProvider.notifier),
        signedEventRelay: SignedEventRelay(
          session: ref.read(relaySessionProvider.notifier),
          nsec: null,
        ),
        currentPubkey: _me,
      );

  final removed = <String>[];
  final left = <String>[];
  final removedBeforeLeave = <List<String>>[];

  @override
  Future<void> removeMember({
    required String channelId,
    required String pubkey,
  }) async => removed.add(pubkey);

  @override
  Future<void> leaveChannel(String channelId) async => left.add(channelId);

  @override
  Future<void> removeAgentsAndLeave({
    required String channelId,
    required Iterable<String> agentPubkeys,
  }) async {
    removedBeforeLeave.add(agentPubkeys.toList());
    left.add(channelId);
  }
}

Widget _agentsApp(void Function(_RecordingActions) onActions) => ProviderScope(
  overrides: [
    myPubkeyProvider.overrideWithValue(_me),
    currentPubkeyProvider.overrideWith((ref) => _me),
    channelMembersProvider('chat-1').overrideWith(
      (ref) async => [
        for (final pubkey in [_me, _ann, _myAgent, _bobsAgent])
          ChannelMember(
            pubkey: pubkey,
            role: pubkey == _myAgent || pubkey == _bobsAgent ? 'bot' : 'member',
            joinedAt: DateTime(2026),
          ),
      ],
    ),
    agentOwnersProvider.overrideWithValue(
      AsyncValue.data({_myAgent: _me, _bobsAgent: _bob}),
    ),
    channelNamesProvider.overrideWith(_FakeNames.new),
    channelActionsProvider.overrideWith((ref) {
      final actions = _RecordingActions(ref);
      onActions(actions);
      return actions;
    }),
  ],
  child: MaterialApp(
    theme: AppTheme.light(),
    home: Scaffold(body: ChatInfoSheet(channel: _chat())),
  ),
);

Channel _chat({String name = ''}) => Channel(
  id: 'chat-1',
  name: name,
  channelType: 'stream',
  visibility: 'private',
  description: '',
  createdBy: '',
  createdAt: DateTime(2026),
  memberCount: 3,
  isMember: true,
  participantPubkeys: [_me, _ann, _bob],
  groupChat: true,
);

Channel _dm() => Channel(
  id: 'dm-1',
  name: '',
  channelType: 'dm',
  visibility: 'private',
  description: '',
  createdBy: '',
  createdAt: DateTime(2026),
  memberCount: 2,
  isMember: true,
  participantPubkeys: [_me, _ann],
);

class _FakeNames extends ChannelNamesNotifier {
  final calls = <(String, String?)>[];

  @override
  ChannelNameStore build() => const ChannelNameStore();

  @override
  void setName(String channelId, String? name) {
    calls.add((channelId, name));
    state = ChannelNameStore(
      channels: {
        ...state.channels,
        channelId: ChannelNameEntry(
          name: normalizeChannelName(name),
          updatedAt: 1,
        ),
      },
    );
  }
}

List<ChannelMember> _members() => [
  for (final pubkey in [_me, _ann, _bob])
    ChannelMember(
      pubkey: pubkey,
      role: pubkey == _me ? 'owner' : 'member',
      joinedAt: DateTime(2026),
    ),
];

Widget _app(Widget child, {required _FakeNames names, Channel? channel}) =>
    ProviderScope(
      overrides: [
        myPubkeyProvider.overrideWithValue(_me),
        currentPubkeyProvider.overrideWith((ref) => _me),
        channelMembersProvider(
          (channel ?? _chat()).id,
        ).overrideWith((ref) async => _members()),
        agentOwnersProvider.overrideWithValue(
          const AsyncValue.data(<String, String>{}),
        ),
        channelNamesProvider.overrideWith(() => names),
      ],
      child: MaterialApp(
        theme: AppTheme.light(),
        home: Scaffold(body: child),
      ),
    );

void main() {
  testWidgets('Chat info: title, N people, my settings, no admin controls', (
    tester,
  ) async {
    final names = _FakeNames();
    await tester.pumpWidget(
      _app(
        ChatInfoSheet(channel: _chat(name: 'Trip')),
        names: names,
      ),
    );
    await tester.pumpAndSettle();

    expect(find.byKey(const ValueKey('chat-grid-avatar')), findsOneWidget);
    expect(find.text('Trip'), findsOneWidget);
    expect(find.text('3 people'), findsOneWidget);
    for (final label in [
      'Add people',
      'Rename for me',
      'Mute notifications',
      'Goals',
    ]) {
      expect(find.text(label), findsOneWidget, reason: label);
    }
    expect(find.text('Restore name'), findsNothing);
    for (final pubkey in [_me, _ann, _bob]) {
      expect(find.byKey(ValueKey('chat-info-member-$pubkey')), findsOneWidget);
    }
    // Every member is equal: no owner or admin wording or controls.
    expect(find.textContaining('Owner'), findsNothing);
    expect(find.textContaining('Admin'), findsNothing);
    expect(find.textContaining('group'), findsNothing);

    await tester.scrollUntilVisible(
      find.text('Leave chat'),
      200,
      scrollable: find.byType(Scrollable).first,
    );
    expect(find.text('Leave chat'), findsOneWidget);
  });

  testWidgets('Rename for me sets a private name; Restore clears it', (
    tester,
  ) async {
    final names = _FakeNames();
    await tester.pumpWidget(
      _app(
        ChatInfoSheet(channel: _chat(name: 'Trip')),
        names: names,
      ),
    );
    await tester.pumpAndSettle();

    await tester.tap(find.byKey(const ValueKey('chat-info-rename')));
    await tester.pumpAndSettle();
    await tester.enterText(
      find.byKey(const ValueKey('chat-info-rename-field')),
      'Family',
    );
    await tester.tap(find.byKey(const ValueKey('chat-info-rename-save')));
    await tester.pumpAndSettle();

    expect(names.calls, [('chat-1', 'Family')]);
    expect(find.text('Family'), findsOneWidget);
    expect(find.text('Restore name'), findsOneWidget);

    await tester.tap(find.byKey(const ValueKey('chat-info-restore-name')));
    await tester.pumpAndSettle();
    expect(names.calls.last, ('chat-1', null));
    expect(find.text('Trip'), findsOneWidget);
  });

  testWidgets('many members get a search field', (tester) async {
    final many = [
      for (var i = 0; i < 10; i++) i.toRadixString(16).padLeft(64, '1'),
    ];
    await tester.pumpWidget(
      ProviderScope(
        overrides: [
          myPubkeyProvider.overrideWithValue(_me),
          channelMembersProvider('chat-1').overrideWith(
            (ref) async => [
              for (final pubkey in many)
                ChannelMember(
                  pubkey: pubkey,
                  role: 'member',
                  joinedAt: DateTime(2026),
                ),
            ],
          ),
          channelNamesProvider.overrideWith(_FakeNames.new),
        ],
        child: MaterialApp(
          theme: AppTheme.light(),
          home: Scaffold(body: ChatInfoSheet(channel: _chat())),
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('10 people'), findsOneWidget);
    final search = find.byKey(const ValueKey('chat-info-member-search'));
    await tester.scrollUntilVisible(
      search,
      200,
      scrollable: find.byType(Scrollable).first,
    );
    await tester.enterText(search, 'no such person');
    await tester.pumpAndSettle();
    for (final pubkey in many) {
      expect(find.byKey(ValueKey('chat-info-member-$pubkey')), findsNothing);
    }
  });

  testWidgets('I can remove my own agent, and nobody else\'s', (tester) async {
    late _RecordingActions actions;
    await tester.pumpWidget(_agentsApp((value) => actions = value));
    await tester.pumpAndSettle();

    final remove = find.byKey(ValueKey('chat-info-remove-$_myAgent'));
    await tester.scrollUntilVisible(
      remove,
      200,
      scrollable: find.byType(Scrollable).first,
    );
    expect(find.byKey(ValueKey('chat-info-remove-$_bobsAgent')), findsNothing);
    expect(find.byKey(ValueKey('chat-info-remove-$_ann')), findsNothing);
    expect(find.byKey(ValueKey('chat-info-remove-$_me')), findsNothing);

    await tester.tap(remove);
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('chat-info-remove-confirm')));
    await tester.pumpAndSettle();
    expect(actions.removed, [_myAgent]);
  });

  testWidgets('leaving a chat can remove my agents first', (tester) async {
    late _RecordingActions actions;
    await tester.pumpWidget(_agentsApp((value) => actions = value));
    await tester.pumpAndSettle();

    final leave = find.byKey(const ValueKey('chat-info-leave'));
    await tester.scrollUntilVisible(
      leave,
      200,
      scrollable: find.byType(Scrollable).first,
    );
    await tester.tap(leave);
    await tester.pumpAndSettle();
    expect(find.text('Leave and remove my agent'), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('leave-remove-agents')));
    await tester.pumpAndSettle();
    expect(actions.removedBeforeLeave, [
      [_myAgent],
    ]);
    expect(actions.left, ['chat-1']);
  });

  testWidgets('a chat menu offers Chat info and Leave chat only', (
    tester,
  ) async {
    await tester.pumpWidget(
      _app(
        ChannelActionsSheet(channel: _chat(), isUnread: false),
        names: _FakeNames(),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('Chat info'), findsOneWidget);
    expect(find.text('Mute chat'), findsOneWidget);
    expect(find.text('Leave chat'), findsOneWidget);
    for (final label in [
      'Manage group',
      'Archive group',
      'Delete group',
      'Leave group',
      'Copy chat name',
    ]) {
      expect(find.text(label), findsNothing, reason: label);
    }
  });

  testWidgets('a 1:1 menu offers Add people', (tester) async {
    await tester.pumpWidget(
      _app(
        ChannelActionsSheet(channel: _dm(), isUnread: false),
        names: _FakeNames(),
        channel: _dm(),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('dm-add-people')), findsOneWidget);
    expect(find.text('Chat info'), findsNothing);
  });
}
