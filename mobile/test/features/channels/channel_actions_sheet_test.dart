import 'dart:async';

import 'package:kuzz/features/channels/channel.dart';
import 'package:kuzz/features/channels/channel_actions_sheet.dart';
import 'package:kuzz/features/channels/channel_management_provider.dart';
import 'package:kuzz/features/channels/channel_sections/channel_sections_provider.dart';
import 'package:kuzz/features/channels/channel_sections/channel_sections_storage.dart';
import 'package:kuzz/features/channels/manage_channel_sheet.dart';
import 'package:kuzz/shared/mentions/agent_identity_provider.dart';
import 'package:kuzz/shared/relay/relay.dart';
import 'package:kuzz/shared/theme/theme.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

const _currentPubkey = 'me';

Channel _channel({
  String type = 'stream',
  bool isArchived = false,
  bool isMember = true,
  bool goalsEnabled = true,
  List<String> participantPubkeys = const [],
  String? picture,
}) => Channel(
  id: 'channel-id',
  name: type == 'dm' ? 'Alice' : 'general',
  channelType: type,
  visibility: 'open',
  description: '',
  createdBy: 'owner',
  createdAt: DateTime(2025),
  memberCount: 2,
  isMember: isMember,
  archivedAt: isArchived ? DateTime(2025, 1, 2) : null,
  goalsEnabled: goalsEnabled,
  participantPubkeys: participantPubkeys,
  picture: picture,
);

Widget _app({
  required Channel channel,
  required Future<List<ChannelMember>> Function() loadMembers,
  bool isUnread = false,
  AsyncValue<Map<String, String>> agentOwners = const AsyncValue.data(
    <String, String>{},
  ),
  ChannelActions Function(Ref ref)? createChannelActions,
  String? currentPubkey = _currentPubkey,
}) => ProviderScope(
  overrides: [
    currentPubkeyProvider.overrideWith((ref) => currentPubkey),
    channelMembersProvider(channel.id).overrideWith((ref) => loadMembers()),
    agentOwnersProvider.overrideWithValue(agentOwners),
    if (createChannelActions != null)
      channelActionsProvider.overrideWith(createChannelActions),
  ],
  child: MaterialApp(
    theme: AppTheme.light(),
    home: Scaffold(
      body: ChannelActionsSheet(channel: channel, isUnread: isUnread),
    ),
  ),
);

Widget _modalApp({
  required Channel channel,
  required Future<List<ChannelMember>> Function() loadMembers,
  required ChannelActions Function(Ref ref) createChannelActions,
}) => ProviderScope(
  overrides: [
    currentPubkeyProvider.overrideWith((ref) => _currentPubkey),
    channelMembersProvider(channel.id).overrideWith((ref) => loadMembers()),
    channelSectionsProvider.overrideWith(
      () => _FakeChannelSectionsNotifier(const ChannelSectionStore()),
    ),
    agentOwnersProvider.overrideWithValue(
      const AsyncValue.data(<String, String>{}),
    ),
    channelCanvasProvider(channel.id).overrideWith(
      (ref) async => const ChannelCanvas(
        content: null,
        updatedAt: null,
        authorPubkey: null,
      ),
    ),
    channelActionsProvider.overrideWith(createChannelActions),
  ],
  child: MaterialApp(
    theme: AppTheme.light(),
    home: Builder(
      builder: (context) => Scaffold(
        body: TextButton(
          onPressed: () => showChannelActionsSheet(
            context: context,
            channel: channel,
            isUnread: false,
          ),
          child: const Text('Open actions'),
        ),
      ),
    ),
  ),
);

void main() {
  testWidgets('owner sees the complete regular-channel action set', (
    tester,
  ) async {
    await tester.pumpWidget(
      _app(
        channel: _channel(),
        loadMembers: () async => [
          ChannelMember(
            pubkey: _currentPubkey,
            role: 'owner',
            joinedAt: DateTime(2025),
          ),
        ],
      ),
    );
    await tester.pumpAndSettle();

    for (final label in [
      'Pin',
      'Mark Unread',
      'Mute group',
      'Manage group',
      'Copy group name',
      'Copy group ID',
      'Leave group',
      'Archive group',
      'Delete group',
    ]) {
      expect(find.text(label), findsOneWidget, reason: label);
    }

    final muteTop = tester.getTopLeft(find.text('Mute group')).dy;
    final manageTop = tester.getTopLeft(find.text('Manage group')).dy;
    final copyNameTop = tester.getTopLeft(find.text('Copy group name')).dy;
    final copyIdTop = tester.getTopLeft(find.text('Copy group ID')).dy;
    expect(muteTop, lessThan(manageTop));
    expect(manageTop, lessThan(copyNameTop));
    expect(copyNameTop, lessThan(copyIdTop));
  });

  testWidgets('unread channel uses the Mark Read label', (tester) async {
    await tester.pumpWidget(
      _app(
        channel: _channel(),
        isUnread: true,
        loadMembers: () async => const [],
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('Mark Read'), findsOneWidget);
    expect(find.text('Mark Unread'), findsNothing);
  });

  testWidgets('admin can archive but cannot delete', (tester) async {
    await tester.pumpWidget(
      _app(
        channel: _channel(),
        loadMembers: () async => [
          ChannelMember(
            pubkey: _currentPubkey,
            role: 'admin',
            joinedAt: DateTime(2025),
          ),
        ],
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('Archive group'), findsOneWidget);
    expect(find.text('Delete group'), findsNothing);
  });

  testWidgets('verified owner agent grants archive and delete', (tester) async {
    const agentPubkey = 'agent';
    await tester.pumpWidget(
      _app(
        channel: _channel(),
        agentOwners: const AsyncValue.data({agentPubkey: _currentPubkey}),
        loadMembers: () async => [
          ChannelMember(
            pubkey: agentPubkey,
            role: 'owner',
            joinedAt: DateTime(2025),
          ),
        ],
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('Archive group'), findsOneWidget);
    expect(find.text('Delete group'), findsOneWidget);
  });

  testWidgets('unresolved identity grants no lifecycle actions', (
    tester,
  ) async {
    await tester.pumpWidget(
      _app(
        channel: _channel(),
        currentPubkey: null,
        loadMembers: () async => [
          ChannelMember(
            pubkey: 'ordinary-owner',
            role: 'owner',
            joinedAt: DateTime(2025),
          ),
        ],
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('Archive group'), findsNothing);
    expect(find.text('Delete group'), findsNothing);
  });

  testWidgets('archived owner can unarchive but cannot delete', (tester) async {
    late _FakeChannelActions actions;
    await tester.pumpWidget(
      _app(
        channel: _channel(isArchived: true),
        loadMembers: () async => [
          ChannelMember(
            pubkey: _currentPubkey,
            role: 'owner',
            joinedAt: DateTime(2025),
          ),
        ],
        createChannelActions: (ref) => actions = _FakeChannelActions(ref),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('Archive group'), findsNothing);
    expect(find.text('Unarchive group'), findsOneWidget);
    expect(find.text('Delete group'), findsNothing);

    await tester.tap(find.text('Unarchive group'));
    await tester.pumpAndSettle();
    expect(find.text('Unarchive general?'), findsOneWidget);
    await tester.tap(find.widgetWithText(FilledButton, 'Unarchive'));
    await tester.pumpAndSettle();

    expect(actions.unarchivedChannelId, 'channel-id');
  });

  testWidgets('owned non-owner agent grants no lifecycle actions', (
    tester,
  ) async {
    const agentPubkey = 'agent';
    await tester.pumpWidget(
      _app(
        channel: _channel(),
        agentOwners: const AsyncValue.data({agentPubkey: _currentPubkey}),
        loadMembers: () async => [
          ChannelMember(
            pubkey: agentPubkey,
            role: 'bot',
            joinedAt: DateTime(2025),
          ),
        ],
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('Archive group'), findsNothing);
    expect(find.text('Delete group'), findsNothing);
  });

  testWidgets('agent ownership loading keeps lifecycle actions pending', (
    tester,
  ) async {
    await tester.pumpWidget(
      _app(
        channel: _channel(),
        agentOwners: const AsyncValue.loading(),
        loadMembers: () async => const [],
      ),
    );
    await tester.pump();

    expect(find.text('Loading group actions…'), findsOneWidget);
    expect(find.text('Archive group'), findsNothing);
    expect(find.text('Delete group'), findsNothing);
  });

  testWidgets('member sees neither owner action', (tester) async {
    await tester.pumpWidget(
      _app(
        channel: _channel(),
        loadMembers: () async => [
          ChannelMember(
            pubkey: _currentPubkey,
            role: 'member',
            joinedAt: DateTime(2025),
          ),
        ],
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('Archive group'), findsNothing);
    expect(find.text('Delete group'), findsNothing);
    expect(find.text('Move to section…'), findsNothing);
    expect(find.text('Leave group'), findsOneWidget);
  });

  testWidgets('shows loading and unavailable capability states', (
    tester,
  ) async {
    final pending = Completer<List<ChannelMember>>();
    await tester.pumpWidget(
      _app(channel: _channel(), loadMembers: () => pending.future),
    );
    await tester.pump();
    expect(find.text('Loading group actions…'), findsOneWidget);

    pending.completeError(Exception('relay unavailable'));
    await tester.pumpAndSettle();
    expect(find.text('Group actions unavailable'), findsOneWidget);
  });

  testWidgets('Manage contains editing and canvas, not mute or leave', (
    tester,
  ) async {
    await tester.pumpWidget(
      _modalApp(
        channel: _channel(),
        loadMembers: () async => const [],
        createChannelActions: (ref) => _FakeChannelActions(ref),
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.text('Open actions'));
    await tester.pumpAndSettle();

    await tester.tap(find.text('Manage group'));
    await tester.pumpAndSettle();

    final manageSheet = find.byType(ManageChannelSheet);
    expect(manageSheet, findsOneWidget);
    expect(
      find.descendant(
        of: manageSheet,
        matching: find.byKey(const ValueKey('manage-channel-name')),
      ),
      findsOneWidget,
    );
    expect(
      find.descendant(of: manageSheet, matching: find.text('Canvas')),
      findsOneWidget,
    );
    expect(
      find.descendant(of: manageSheet, matching: find.text('Mute group')),
      findsNothing,
    );
    expect(
      find.descendant(of: manageSheet, matching: find.text('Leave group')),
      findsNothing,
    );
    expect(
      find.descendant(of: manageSheet, matching: find.text('Topic')),
      findsNothing,
    );
    expect(
      find.descendant(of: manageSheet, matching: find.text('Purpose')),
      findsNothing,
    );
  });

  testWidgets('Manage refreshes metadata after saving from actions', (
    tester,
  ) async {
    await tester.pumpWidget(
      _modalApp(
        channel: _channel(),
        loadMembers: () async => [
          ChannelMember(
            pubkey: _currentPubkey,
            role: 'owner',
            joinedAt: DateTime(2025),
          ),
        ],
        createChannelActions: (ref) => _FakeChannelActions(
          ref,
          onUpdateChannel: (channelId, name, description) async {},
        ),
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.text('Open actions'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Manage group'));
    await tester.pumpAndSettle();

    await tester.enterText(
      find.byKey(const ValueKey('manage-channel-name')),
      'renamed',
    );
    await tester.ensureVisible(
      find.byKey(const ValueKey('manage-channel-save-details')),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('manage-channel-save-details')));
    await tester.pumpAndSettle();

    expect(find.widgetWithText(ListTile, 'Manage group'), findsOneWidget);
    await tester.tap(find.widgetWithText(ListTile, 'Manage group'));
    await tester.pumpAndSettle();
    final nameField = tester.widget<TextField>(
      find.byKey(const ValueKey('manage-channel-name')),
    );
    expect(nameField.controller?.text, 'renamed');
  });

  testWidgets('non-member cannot edit canvas from Manage channel', (
    tester,
  ) async {
    await tester.pumpWidget(
      _modalApp(
        channel: _channel(isMember: false),
        loadMembers: () async => const [],
        createChannelActions: (ref) => _FakeChannelActions(ref),
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.text('Open actions'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Manage group'));
    await tester.pumpAndSettle();

    final editCanvas = find.widgetWithText(FilledButton, 'Create canvas');
    expect(editCanvas, findsOneWidget);
    expect(tester.widget<FilledButton>(editCanvas).onPressed, isNull);
  });

  testWidgets('regular members can pin chats but cannot edit metadata', (
    tester,
  ) async {
    await tester.pumpWidget(
      _modalApp(
        channel: _channel(),
        loadMembers: () async => [
          ChannelMember(
            pubkey: _currentPubkey,
            role: 'member',
            joinedAt: DateTime(2025),
          ),
        ],
        createChannelActions: (ref) => _FakeChannelActions(ref),
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.text('Open actions'));
    await tester.pumpAndSettle();

    expect(find.text('Move to section…'), findsNothing);
    await tester.tap(find.text('Manage group'));
    await tester.pumpAndSettle();

    final nameField = find.byKey(const ValueKey('manage-channel-name'));
    final descriptionField = find.byKey(
      const ValueKey('manage-channel-description'),
    );
    expect(tester.widget<TextField>(nameField).enabled, isFalse);
    expect(tester.widget<TextField>(descriptionField).enabled, isFalse);
    expect(
      tester
          .widget<FilledButton>(
            find.byKey(const ValueKey('manage-channel-save-details')),
          )
          .onPressed,
      isNull,
    );
    expect(
      tester
          .widget<FilledButton>(
            find.widgetWithText(FilledButton, 'Create canvas'),
          )
          .onPressed,
      isNotNull,
    );
  });

  testWidgets('DM offers pin and read actions with mute and copy rows', (
    tester,
  ) async {
    await tester.pumpWidget(
      _app(
        channel: _channel(type: 'dm'),
        loadMembers: () async => const [],
      ),
    );
    await tester.pumpAndSettle();

    for (final label in [
      'Pin',
      'Mark Unread',
      'Mute chat',
      'Copy chat name',
      'Copy chat ID',
    ]) {
      expect(find.text(label), findsOneWidget, reason: label);
    }
    for (final label in [
      'Unpin',
      'Mark Read',
      'Move to section…',
      'Manage group',
      'Leave group',
      'Archive group',
      'Delete group',
    ]) {
      expect(find.text(label), findsNothing, reason: label);
    }

    final muteTop = tester.getTopLeft(find.text('Mute chat')).dy;
    final copyNameTop = tester.getTopLeft(find.text('Copy chat name')).dy;
    final copyIdTop = tester.getTopLeft(find.text('Copy chat ID')).dy;
    expect(muteTop, lessThan(copyNameTop));
    expect(copyNameTop, lessThan(copyIdTop));
  });

  group('DM goals switch', () {
    final goalsSwitch = find.byKey(const ValueKey('room-goals-switch'));

    testWidgets('DM participant turns goals off immediately', (tester) async {
      late _FakeChannelActions actions;
      await tester.pumpWidget(
        _app(
          channel: _channel(type: 'dm'),
          loadMembers: () async => const [],
          createChannelActions: (ref) => actions = _FakeChannelActions(ref),
        ),
      );
      await tester.pumpAndSettle();

      expect(tester.widget<SwitchListTile>(goalsSwitch).value, isTrue);
      expect(find.bySemanticsLabel(RegExp(r'^Goals')), findsOneWidget);
      await tester.tap(goalsSwitch);
      await tester.pumpAndSettle();

      expect(actions.goalsUpdates, [false]);
      expect(tester.widget<SwitchListTile>(goalsSwitch).value, isFalse);
    });

    testWidgets('failed goals save reverts and reports', (tester) async {
      late _FakeChannelActions actions;
      await tester.pumpWidget(
        _app(
          channel: _channel(type: 'dm', goalsEnabled: false),
          loadMembers: () async => const [],
          createChannelActions: (ref) =>
              actions = _FakeChannelActions(ref)
                ..goalsUpdateError = StateError('rejected'),
        ),
      );
      await tester.pumpAndSettle();

      await tester.tap(goalsSwitch);
      await tester.pumpAndSettle();

      expect(actions.goalsUpdates, [true]);
      expect(tester.widget<SwitchListTile>(goalsSwitch).value, isFalse);
      expect(find.text('Couldn’t update goals. Try again.'), findsOneWidget);
    });

    testWidgets('participant listed by pubkey may change goals', (
      tester,
    ) async {
      await tester.pumpWidget(
        _app(
          channel: _channel(
            type: 'dm',
            isMember: false,
            participantPubkeys: const ['ME', 'alice'],
          ),
          loadMembers: () async => const [],
        ),
      );
      await tester.pumpAndSettle();

      expect(tester.widget<SwitchListTile>(goalsSwitch).onChanged, isNotNull);
    });

    testWidgets('non-participant and archived DMs are read-only', (
      tester,
    ) async {
      await tester.pumpWidget(
        _app(
          channel: _channel(type: 'dm', isMember: false),
          loadMembers: () async => const [],
        ),
      );
      await tester.pumpAndSettle();
      expect(tester.widget<SwitchListTile>(goalsSwitch).onChanged, isNull);

      await tester.pumpWidget(const SizedBox());
      await tester.pumpWidget(
        _app(
          channel: _channel(type: 'dm', isArchived: true),
          loadMembers: () async => const [],
        ),
      );
      await tester.pumpAndSettle();
      expect(tester.widget<SwitchListTile>(goalsSwitch).onChanged, isNull);
    });

    testWidgets('regular channels keep goals in channel details', (
      tester,
    ) async {
      await tester.pumpWidget(
        _app(
          channel: _channel(),
          loadMembers: () async => [
            ChannelMember(
              pubkey: _currentPubkey,
              role: 'owner',
              joinedAt: DateTime(2025),
            ),
          ],
        ),
      );
      await tester.pumpAndSettle();

      expect(goalsSwitch, findsNothing);
    });
  });

  group('earlier messages in Manage group', () {
    late _FakeChannelActions actions;
    Channel? updated;

    Future<void> pumpSheet(
      WidgetTester tester, {
      required Channel channel,
      required bool canEditDetails,
    }) async {
      updated = null;
      await tester.pumpWidget(
        ProviderScope(
          overrides: [
            currentPubkeyProvider.overrideWith((ref) => _currentPubkey),
            channelCanvasProvider(channel.id).overrideWith(
              (ref) async => const ChannelCanvas(
                content: null,
                updatedAt: null,
                authorPubkey: null,
              ),
            ),
            channelActionsProvider.overrideWith(
              (ref) => actions = _FakeChannelActions(ref),
            ),
          ],
          child: MaterialApp(
            theme: AppTheme.light(),
            home: Scaffold(
              body: ManageChannelSheet(
                channel: channel,
                canEditDetails: canEditDetails,
                onChannelUpdated: (channel) => updated = channel,
              ),
            ),
          ),
        ),
      );
      await tester.pump();
    }

    final historySwitch = find.byKey(const ValueKey('group-history-switch'));

    testWidgets('owners switch it on and off with a history tag', (
      tester,
    ) async {
      await pumpSheet(tester, channel: _channel(), canEditDetails: true);

      expect(find.text('New members can see earlier messages'), findsOneWidget);
      expect(tester.widget<SwitchListTile>(historySwitch).value, isFalse);

      await tester.tap(historySwitch);
      await tester.pumpAndSettle();
      expect(actions.historyUpdates, [true]);
      expect(updated?.historyAll, isTrue);
      expect(tester.widget<SwitchListTile>(historySwitch).value, isTrue);

      await tester.tap(historySwitch);
      await tester.pumpAndSettle();
      expect(actions.historyUpdates, [true, false]);
      expect(updated?.historyAll, isFalse);
    });

    testWidgets('reads the current value from the group', (tester) async {
      await pumpSheet(
        tester,
        channel: _channel().copyWith(historyAll: true),
        canEditDetails: true,
      );
      expect(tester.widget<SwitchListTile>(historySwitch).value, isTrue);
    });

    testWidgets('members without owner or admin rights do not see it', (
      tester,
    ) async {
      await pumpSheet(tester, channel: _channel(), canEditDetails: false);
      expect(historySwitch, findsNothing);
    });
  });

  group('leaving with my agents', () {
    const agent = 'my-agent';
    const otherAgent = 'their-agent';
    late _FakeChannelActions actions;

    List<ChannelMember> members({String myRole = 'member'}) => [
      ChannelMember(
        pubkey: _currentPubkey,
        role: myRole,
        joinedAt: DateTime(2025),
      ),
      ChannelMember(pubkey: 'owner', role: 'owner', joinedAt: DateTime(2025)),
      ChannelMember(pubkey: agent, role: 'bot', joinedAt: DateTime(2025)),
      ChannelMember(pubkey: otherAgent, role: 'bot', joinedAt: DateTime(2025)),
    ];

    Future<void> openLeave(
      WidgetTester tester,
      List<ChannelMember> roster,
    ) async {
      await tester.pumpWidget(
        _app(
          channel: _channel(),
          loadMembers: () async => roster,
          agentOwners: const AsyncValue.data({
            agent: _currentPubkey,
            otherAgent: 'someone-else',
          }),
          createChannelActions: (ref) => actions = _FakeChannelActions(ref),
        ),
      );
      await tester.pumpAndSettle();
      await tester.tap(find.text('Leave group'));
      await tester.pumpAndSettle();
    }

    testWidgets('offers removing only my agents, then leaves', (tester) async {
      await openLeave(tester, members());

      expect(find.text('Leave and remove my agent'), findsOneWidget);
      expect(find.text('Leave only'), findsOneWidget);
      expect(find.text('Cancel'), findsOneWidget);

      await tester.tap(find.byKey(const ValueKey('leave-remove-agents')));
      await tester.pumpAndSettle();
      expect(actions.removedAgentsBeforeLeave, [
        [agent],
      ]);
      expect(actions.leftChannelIds, ['channel-id']);
    });

    testWidgets('Leave only keeps my agents', (tester) async {
      await openLeave(tester, members());
      await tester.tap(find.byKey(const ValueKey('leave-only')));
      await tester.pumpAndSettle();
      expect(actions.removedAgentsBeforeLeave, isEmpty);
      expect(actions.leftChannelIds, ['channel-id']);
    });

    testWidgets('Cancel does nothing', (tester) async {
      await openLeave(tester, members());
      await tester.tap(find.byKey(const ValueKey('leave-cancel')));
      await tester.pumpAndSettle();
      expect(actions.removedAgentsBeforeLeave, isEmpty);
      expect(actions.leftChannelIds, isEmpty);
    });

    testWidgets('the only owner gets the plain confirmation', (tester) async {
      await openLeave(
        tester,
        members(
          myRole: 'owner',
        ).where((member) => member.pubkey != 'owner').toList(),
      );
      expect(find.textContaining('remove my agents'), findsNothing);
      await tester.tap(find.widgetWithText(FilledButton, 'Leave'));
      await tester.pumpAndSettle();
      expect(actions.removedAgentsBeforeLeave, isEmpty);
      expect(actions.leftChannelIds, ['channel-id']);
    });

    testWidgets('without my agents the confirmation stays plain', (
      tester,
    ) async {
      await openLeave(
        tester,
        members().where((member) => member.pubkey != agent).toList(),
      );
      expect(find.textContaining('remove my agents'), findsNothing);
      expect(find.widgetWithText(FilledButton, 'Leave'), findsOneWidget);
    });
  });

  group('group photo in Manage group', () {
    const photoUrl = 'https://relay.example/media/photo.jpg';
    late _FakeChannelActions actions;
    Channel? updated;

    Future<void> pumpSheet(
      WidgetTester tester, {
      required Channel channel,
      required bool canEditDetails,
    }) async {
      updated = null;
      await tester.pumpWidget(
        ProviderScope(
          overrides: [
            currentPubkeyProvider.overrideWith((ref) => _currentPubkey),
            channelCanvasProvider(channel.id).overrideWith(
              (ref) async => const ChannelCanvas(
                content: null,
                updatedAt: null,
                authorPubkey: null,
              ),
            ),
            channelActionsProvider.overrideWith(
              (ref) => actions = _FakeChannelActions(ref),
            ),
          ],
          child: MaterialApp(
            theme: AppTheme.light(),
            home: Scaffold(
              body: ManageChannelSheet(
                channel: channel,
                canEditDetails: canEditDetails,
                onChannelUpdated: (channel) => updated = channel,
              ),
            ),
          ),
        ),
      );
      await tester.pump();
    }

    testWidgets('owners see the photo with Change and Remove on top', (
      tester,
    ) async {
      await pumpSheet(
        tester,
        channel: _channel(picture: photoUrl),
        canEditDetails: true,
      );

      expect(
        find.byKey(const ValueKey('manage-channel-photo')),
        findsOneWidget,
      );
      expect(find.text('Change photo'), findsOneWidget);
      expect(find.text('Remove photo'), findsOneWidget);
      expect(
        tester
            .getTopLeft(find.byKey(const ValueKey('manage-channel-photo')))
            .dy,
        lessThan(
          tester
              .getTopLeft(find.byKey(const ValueKey('manage-channel-name')))
              .dy,
        ),
      );
    });

    testWidgets('without a photo only Change photo shows', (tester) async {
      await pumpSheet(tester, channel: _channel(), canEditDetails: true);

      expect(find.text('Change photo'), findsOneWidget);
      expect(find.text('Remove photo'), findsNothing);
      expect(find.text('G'), findsOneWidget);
    });

    testWidgets('members who cannot edit see no photo controls', (
      tester,
    ) async {
      await pumpSheet(
        tester,
        channel: _channel(picture: photoUrl),
        canEditDetails: false,
      );

      expect(find.byKey(const ValueKey('manage-channel-photo')), findsNothing);
      expect(find.text('Change photo'), findsNothing);
      expect(find.text('Remove photo'), findsNothing);
    });

    testWidgets('Remove photo sends an empty picture and shows the letter', (
      tester,
    ) async {
      await pumpSheet(
        tester,
        channel: _channel(picture: photoUrl),
        canEditDetails: true,
      );

      await tester.tap(find.text('Remove photo'));
      await tester.pump();
      await tester.pump();

      expect(actions.pictureUpdates, ['']);
      expect(updated?.picture, isNull);
      expect(updated?.name, 'general');
      expect(find.text('Remove photo'), findsNothing);
      expect(find.text('G'), findsOneWidget);
    });
  });
}

class _FakeChannelSectionsNotifier extends ChannelSectionsNotifier {
  _FakeChannelSectionsNotifier(this._store);

  final ChannelSectionStore _store;

  @override
  ChannelSectionsState build() =>
      ChannelSectionsState(isReady: true, store: _store, version: 1);
}

class _FakeChannelActions extends ChannelActions {
  final Future<void> Function(
    String channelId,
    String? name,
    String? description,
  )?
  onUpdateChannel;

  _FakeChannelActions(Ref ref, {this.onUpdateChannel})
    : super(
        ref: ref,
        session: ref.read(relaySessionProvider.notifier),
        signedEventRelay: SignedEventRelay(
          session: ref.read(relaySessionProvider.notifier),
          nsec: null,
        ),
        currentPubkey: _currentPubkey,
      );

  @override
  Future<void> updateChannel({
    required String channelId,
    String? name,
    String? description,
    bool? goalsEnabled,
    String? visibility,
    String? picture,
    bool? historyAll,
  }) async {
    if (picture != null) pictureUpdates.add(picture);
    if (historyAll != null) historyUpdates.add(historyAll);
    if (goalsEnabled != null) {
      goalsUpdates.add(goalsEnabled);
      final failure = goalsUpdateError;
      if (failure != null) throw failure;
    }
    await onUpdateChannel?.call(channelId, name, description);
  }

  final List<bool> goalsUpdates = [];
  final List<String> pictureUpdates = [];
  final List<bool> historyUpdates = [];
  Object? goalsUpdateError;

  String? unarchivedChannelId;
  final List<String> leftChannelIds = [];
  final List<List<String>> removedAgentsBeforeLeave = [];

  @override
  Future<void> leaveChannel(String channelId) async {
    leftChannelIds.add(channelId);
  }

  @override
  Future<void> removeAgentsAndLeave({
    required String channelId,
    required Iterable<String> agentPubkeys,
  }) async {
    removedAgentsBeforeLeave.add(agentPubkeys.toList());
    leftChannelIds.add(channelId);
  }

  @override
  Future<void> unarchiveChannel(String channelId) async {
    unarchivedChannelId = channelId;
  }
}
