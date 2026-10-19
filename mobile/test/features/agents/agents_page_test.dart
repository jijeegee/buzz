import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:kuzz/features/agents/agent_settings_page.dart';
import 'package:kuzz/features/agents/my_agent_actions.dart';
import 'package:kuzz/features/agents/agent_team_page.dart';
import 'package:kuzz/features/agents/agents_page.dart';
import 'package:kuzz/features/agents/my_agents.dart';
import 'package:kuzz/features/agents/my_teams.dart';
import 'package:kuzz/features/channels/channel.dart';
import 'package:kuzz/features/channels/send_message_provider.dart';
import 'package:kuzz/features/profile/presence_cache_provider.dart';
import 'package:kuzz/shared/devices/agent_host_devices.dart';
import 'package:kuzz/shared/devices/device_robot.dart';
import 'package:kuzz/shared/mentions/agent_identity_provider.dart';
import 'package:kuzz/shared/profile/user_cache_provider.dart';
import 'package:kuzz/shared/profile/user_profile.dart';
import 'package:kuzz/shared/relay/relay.dart';
import 'package:kuzz/shared/theme/theme.dart';

final _fizz = '1' * 64;
final _honey = '2' * 64;
final _scout = '3' * 64;

void main() {
  Future<_RecordingSend> pump(
    WidgetTester tester, {
    required List<MyAgent> agents,
    Map<String, String> presence = const {},
    List<MyAgentTeam> teams = const [],
    List<MyTeamPreset> teamPresets = const [],
    List<MyAgentPreset> presets = const [],
    String? currentDevice = 'phone',
    List<String>? openedDms,
  }) async {
    final send = _RecordingSend();
    await tester.pumpWidget(
      ProviderScope(
        overrides: [
          myAgentsProvider.overrideWithValue(agents),
          agentDirectoryProvider.overrideWith(
            (ref) async => const <AgentDirectoryEntry>[],
          ),
          myAgentTeamsProvider.overrideWith((ref) async => teams),
          myTeamPresetsProvider.overrideWith((ref) async => teamPresets),
          myAgentPresetsProvider.overrideWith((ref) async => presets),
          presenceCacheProvider.overrideWith(() => _FakePresence(presence)),
          userCacheProvider.overrideWith(
            () => _FakeUsers({
              _fizz: UserProfile(pubkey: _fizz, displayName: 'Fizz'),
              _honey: UserProfile(pubkey: _honey, displayName: 'Honey'),
              _scout: UserProfile(pubkey: _scout, displayName: 'Scout'),
            }),
          ),
          agentHostDeviceNamesProvider.overrideWithValue(const {
            'pc-a': 'Office PC',
            'pc-b': 'Mac mini',
          }),
          currentAccountDeviceIdProvider.overrideWithValue(currentDevice),
          deviceRobotOverridesProvider.overrideWithValue(
            const <String, DeviceRobotVariant>{},
          ),
          sendMessageProvider.overrideWithValue(send),
          myAgentCommandChannelProvider.overrideWithValue(
            (pubkey) async => 'dm-$pubkey',
          ),
          // Records the DM asked for; the DM page itself is not under test.
          myAgentDmProvider.overrideWithValue((pubkey) async {
            openedDms?.add(pubkey);
            throw StateError('offline');
          }),
          // Signed out for settings: they open read-only, with no relay.
          myPubkeyProvider.overrideWithValue(null),
        ],
        child: MaterialApp(theme: AppTheme.light(), home: const AgentsPage()),
      ),
    );
    await tester.pump();
    await tester.pump();
    return send;
  }

  testWidgets('groups the owner\'s agents by computer, sorted by name', (
    tester,
  ) async {
    await pump(
      tester,
      agents: [
        MyAgent(pubkey: _fizz, deviceId: 'pc-a'),
        MyAgent(pubkey: _honey, deviceId: 'pc-b'),
        MyAgent(pubkey: _scout, deviceId: 'pc-a'),
      ],
      presence: {_fizz: 'online', _honey: 'away', _scout: 'offline'},
    );
    final mac = tester.getTopLeft(find.text('Mac mini')).dy;
    final office = tester.getTopLeft(find.text('Office PC')).dy;
    final fizz = tester.getTopLeft(find.text('Fizz')).dy;
    final scout = tester.getTopLeft(find.text('Scout')).dy;
    final honey = tester.getTopLeft(find.text('Honey')).dy;
    expect(mac < honey && honey < office, isTrue);
    expect(office < fizz && fizz < scout, isTrue);
    expect(find.text('Online'), findsOneWidget);
    expect(find.text('Busy'), findsOneWidget);
    expect(find.text('Stopped'), findsOneWidget);
    // Only the owner's agents: no other people's agents, no settings.
    expect(find.text('Other agents'.toUpperCase()), findsNothing);
    expect(find.text('Automatically mention agents'), findsNothing);
  });

  testWidgets('shows how to get agents when there are none', (tester) async {
    await pump(tester, agents: const []);
    expect(find.byKey(const Key('agents-empty')), findsOneWidget);
  });

  testWidgets('the filter narrows agents by name', (tester) async {
    await pump(
      tester,
      agents: [
        MyAgent(pubkey: _fizz, deviceId: 'pc-a'),
        MyAgent(pubkey: _honey, deviceId: 'pc-b'),
      ],
    );
    await tester.enterText(find.byType(TextField), 'hon');
    await tester.pump();
    expect(find.text('Honey'), findsOneWidget);
    expect(find.text('Fizz'), findsNothing);
    expect(find.text('Office PC'), findsNothing);
    await tester.enterText(find.byType(TextField), 'zzz');
    await tester.pump();
    expect(find.byKey(const Key('agents-no-match')), findsOneWidget);
  });

  testWidgets('menu Stop sends the owner command after confirming', (
    tester,
  ) async {
    final send = await pump(
      tester,
      agents: [MyAgent(pubkey: _fizz, deviceId: 'pc-a')],
      presence: {_fizz: 'online'},
    );
    await tester.tap(find.byKey(ValueKey('my-agent-menu-$_fizz')));
    await tester.pumpAndSettle();
    expect(find.byKey(ValueKey('my-agent-menu-start-$_fizz')), findsNothing);
    await tester.tap(find.byKey(ValueKey('my-agent-menu-stop-$_fizz')));
    await tester.pumpAndSettle();
    expect(find.text('Stop Fizz?'), findsOneWidget);
    await tester.tap(find.byKey(const Key('my-agents-confirm-stop')));
    await tester.pumpAndSettle();
    expect(send.calls, [('dm-$_fizz', '!shutdown', _fizz)]);
    expect(find.textContaining('Stop requested'), findsOneWidget);
  });

  testWidgets('menu Start asks the agent\'s computer to start it', (
    tester,
  ) async {
    final send = await pump(
      tester,
      agents: [MyAgent(pubkey: _fizz, deviceId: 'pc-a')],
      presence: {_fizz: 'offline'},
    );
    // A long press opens the same sheet as the "⋮".
    await tester.longPress(find.text('Fizz'));
    await tester.pumpAndSettle();
    expect(find.byKey(ValueKey('my-agent-sheet-$_fizz')), findsOneWidget);
    expect(find.byKey(ValueKey('my-agent-menu-stop-$_fizz')), findsNothing);
    await tester.tap(find.byKey(ValueKey('my-agent-menu-start-$_fizz')));
    await tester.pumpAndSettle();
    expect(send.calls, [('dm-$_fizz', '!start', _fizz)]);
    expect(
      find.text(
        'Start requested. Fizz starts only if Kuzz is open on Office PC now.',
      ),
      findsOneWidget,
    );
  });

  testWidgets('a running agent offers Stop, not Start', (tester) async {
    await pump(
      tester,
      agents: [MyAgent(pubkey: _scout, deviceId: 'pc-a')],
      presence: {_scout: 'online'},
    );
    await tester.tap(find.byKey(ValueKey('my-agent-menu-$_scout')));
    await tester.pumpAndSettle();
    expect(find.byKey(ValueKey('my-agent-menu-stop-$_scout')), findsOneWidget);
    expect(find.byKey(ValueKey('my-agent-menu-start-$_scout')), findsNothing);
    expect(find.text('Online · Office PC'), findsOneWidget);
    for (final action in ['message', 'edit', 'profile']) {
      expect(
        find.byKey(ValueKey('my-agent-menu-$action-$_scout')),
        findsOneWidget,
      );
    }
  });

  testWidgets('tapping an agent opens its DM, not its settings', (
    tester,
  ) async {
    final opened = <String>[];
    await pump(
      tester,
      agents: [MyAgent(pubkey: _fizz, deviceId: 'pc-a')],
      presence: {_fizz: 'online'},
      openedDms: opened,
    );
    await tester.tap(find.text('Fizz'));
    await tester.pumpAndSettle();
    expect(opened, [_fizz]);
    expect(find.byType(AgentSettingsPage), findsNothing);
    expect(
      find.textContaining('Couldn’t open the DM with Fizz'),
      findsOneWidget,
    );
    // Message in the sheet opens the same DM.
    await tester.tap(find.byKey(ValueKey('my-agent-menu-$_fizz')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(ValueKey('my-agent-menu-message-$_fizz')));
    await tester.pumpAndSettle();
    expect(opened, [_fizz, _fizz]);
  });

  testWidgets('sheet Edit opens its settings', (tester) async {
    await pump(
      tester,
      agents: [MyAgent(pubkey: _fizz, deviceId: 'pc-a')],
      presence: {_fizz: 'online'},
    );
    await tester.tap(find.byKey(ValueKey('my-agent-menu-$_fizz')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(ValueKey('my-agent-menu-edit-$_fizz')));
    await tester.pumpAndSettle();
    expect(find.byType(AgentSettingsPage), findsOneWidget);
    // Settings show the agent's state and Stop.
    expect(find.byKey(const Key('my-agent-status-card')), findsOneWidget);
    expect(find.byKey(ValueKey('my-agent-stop-$_fizz')), findsOneWidget);
  });

  testWidgets('settings Start shows Starting… while the command is sent', (
    tester,
  ) async {
    final send = await pump(
      tester,
      agents: [MyAgent(pubkey: _fizz, deviceId: 'pc-a')],
      presence: {_fizz: 'offline'},
    );
    await tester.tap(find.byKey(ValueKey('my-agent-menu-$_fizz')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(ValueKey('my-agent-menu-edit-$_fizz')));
    await tester.pumpAndSettle();
    send.gate = Completer<void>();
    await tester.tap(find.byKey(ValueKey('my-agent-start-$_fizz')));
    await tester.pump();
    expect(find.text('Starting…'), findsOneWidget);
    send.gate!.complete();
    await tester.pumpAndSettle();
    expect(find.text('Start'), findsOneWidget);
  });

  testWidgets('Presets lists agent presets with their use, no status', (
    tester,
  ) async {
    await pump(
      tester,
      agents: [MyAgent(pubkey: _fizz, deviceId: 'pc-a')],
      presence: {_fizz: 'online'},
      presets: const [
        MyAgentPreset(
          id: 'coder',
          name: 'Coder',
          description: 'Works in a repo',
          agentCount: 0,
        ),
        MyAgentPreset(
          id: 'fizz',
          name: 'Fizz',
          description: null,
          agentCount: 2,
        ),
      ],
    );
    // My agents first: real agents, no presets.
    expect(find.text('Online'), findsOneWidget);
    expect(find.text('Coder'), findsNothing);
    expect(find.byKey(const Key('agents-presets-note')), findsNothing);

    await tester.tap(find.byKey(const Key('agents-tab-presets')));
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('agents-presets-note')), findsOneWidget);
    expect(find.text('AGENT PRESETS'), findsOneWidget);
    expect(find.text('Works in a repo · not used yet'), findsOneWidget);
    expect(find.text('used by 2 agents'), findsOneWidget);
    expect(find.text('Online'), findsNothing);
    expect(find.text('Stopped'), findsNothing);

    await tester.enterText(find.byType(TextField), 'cod');
    await tester.pump();
    expect(find.text('Coder'), findsOneWidget);
    expect(find.text('used by 2 agents'), findsNothing);

    await tester.tap(find.byKey(const Key('agents-tab-agents')));
    await tester.pumpAndSettle();
    expect(find.text('AGENT PRESETS'), findsNothing);
  });

  testWidgets('Presets lists team presets, view only', (tester) async {
    await pump(
      tester,
      agents: [MyAgent(pubkey: _fizz, deviceId: 'pc-a')],
      teamPresets: [
        MyTeamPreset(
          id: 'research',
          name: 'Research',
          description: 'Finds and checks sources.',
          members: [
            MyTeamPresetMember(
              personaD: 'fizz',
              name: 'Fizz',
              agentPubkeys: [_fizz],
            ),
            const MyTeamPresetMember(
              personaD: 'writer',
              name: 'Writer',
              agentPubkeys: [],
            ),
          ],
        ),
      ],
    );
    // Team presets are not on My agents.
    expect(find.text('Research'), findsNothing);
    await tester.tap(find.byKey(const Key('agents-tab-presets')));
    await tester.pumpAndSettle();
    expect(find.text('TEAM PRESETS'), findsOneWidget);
    expect(find.text('Research'), findsOneWidget);
    expect(find.text('Fizz · Writer'), findsOneWidget);
    await tester.tap(find.text('Research'));
    await tester.pumpAndSettle();
    expect(find.byType(AgentTeamPage), findsNothing);
  });

  testWidgets('My agents lists teams with status and opens one', (
    tester,
  ) async {
    final gone = '9' * 64;
    await pump(
      tester,
      agents: [
        MyAgent(pubkey: _fizz, deviceId: 'pc-a'),
        MyAgent(pubkey: _honey, deviceId: 'pc-b'),
      ],
      presence: {_fizz: 'online', _honey: 'offline'},
      teams: [
        MyAgentTeam(
          id: 'build',
          name: 'Build crew',
          description: 'Ships the app.',
          agentPubkeys: [_fizz, _honey, gone],
        ),
      ],
    );
    expect(find.text('TEAMS'), findsOneWidget);
    expect(find.text('Build crew'), findsOneWidget);
    expect(find.text('Fizz · Honey · 1 removed'), findsOneWidget);
    expect(
      find.byKey(ValueKey('agents-team-dot-build-$_fizz')),
      findsOneWidget,
    );
    expect(find.byKey(ValueKey('agents-team-dot-build-$gone')), findsNothing);

    // Not on Presets.
    await tester.tap(find.byKey(const Key('agents-tab-presets')));
    await tester.pumpAndSettle();
    expect(find.text('Build crew'), findsNothing);
    await tester.tap(find.byKey(const Key('agents-tab-agents')));
    await tester.pumpAndSettle();

    await tester.tap(find.text('Build crew'));
    await tester.pumpAndSettle();
    expect(find.byType(AgentTeamPage), findsOneWidget);
    expect(find.text('Ships the app.'), findsOneWidget);
    // Agents on different computers, each with its status.
    expect(find.text('Online · Office PC'), findsOneWidget);
    expect(find.text('Stopped · Mac mini'), findsOneWidget);
    expect(find.text('1 agent was removed'), findsOneWidget);
  });
}

class _FakePresence extends PresenceCacheNotifier {
  _FakePresence(this._initial);
  final Map<String, String> _initial;

  @override
  Map<String, String> build() => _initial;

  @override
  void track(List<String> pubkeys) {}
}

class _FakeUsers extends UserCacheNotifier {
  _FakeUsers(this._users);
  final Map<String, UserProfile> _users;

  @override
  Map<String, UserProfile> build() => _users;

  @override
  Future<bool> preload(List<String> pubkeys) async => true;
}

class _RecordingSend implements SendMessage {
  final calls = <(String, String, String)>[];

  /// When set, sends wait for it.
  Completer<void>? gate;

  @override
  Future<void> call({
    required String channelId,
    required String content,
    String? parentEventId,
    String? rootEventId,
    List<String>? mentionPubkeys,
    Channel? channel,
    List<List<String>> mediaTags = const [],
    dynamic quote,
  }) async {
    calls.add((channelId, content, (mentionPubkeys ?? const []).join(',')));
    await gate?.future;
  }

  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}
