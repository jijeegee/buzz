import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../shared/devices/agent_host_devices.dart';
import '../../shared/devices/device_robot.dart';
import '../../shared/devices/device_robot_icon.dart';
import '../../shared/mentions/agent_identity_provider.dart';
import '../../shared/profile/user_cache_provider.dart';
import '../../shared/theme/theme.dart';
import '../../shared/utils/string_utils.dart';
import '../../shared/widgets/frosted_app_bar.dart';
import '../../shared/widgets/frosted_scaffold.dart';
import '../../shared/widgets/kuzz_loading_indicator.dart';
import '../profile/presence_cache_provider.dart';
import 'agent_row.dart';
import 'agent_team_page.dart';
import 'my_agent_actions.dart';
import 'my_agent_tile.dart';
import 'my_agents.dart';
import 'my_teams.dart';

part 'agents_page/computer_header.dart';
part 'agents_page/preset_row.dart';
part 'agents_page/team_row.dart';

/// The Agents tab, where the owner sets up and manages their agents, like
/// desktop's Agents page, with the same two tabs. "My agents": only the
/// owner's real agents, grouped by the computer each runs on (with that
/// computer's robot); tapping one opens its settings, its menu starts, stops
/// or edits it. "Presets": the templates agents are made from, and team
/// presets, with no status. Agents run on computers, so they are created
/// there.
class AgentsPage extends HookConsumerWidget {
  const AgentsPage({this.tabReselection, super.key});

  /// Notifies this page when its already-selected tab is tapped again.
  final ValueListenable<int>? tabReselection;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final agents = ref.watch(myAgentsProvider);
    final users = ref.watch(userCacheProvider);
    final deviceNames = ref.watch(agentHostDeviceNamesProvider);
    final currentDevice = ref.watch(currentAccountDeviceIdProvider);
    final robots = ref.watch(deviceRobotOverridesProvider);
    final tab = useState(_AgentsTab.myAgents);
    final onPresets = tab.value == _AgentsTab.presets;
    final teamsAsync = onPresets
        ? ref.watch(myTeamsProvider)
        : const AsyncValue<List<MyTeam>>.data([]);
    final presetsAsync = onPresets
        ? ref.watch(myAgentPresetsProvider)
        : const AsyncValue<List<MyAgentPreset>>.data([]);
    final directoryLoading = ref.watch(
      agentDirectoryProvider.select((d) => d.isLoading && !d.hasValue),
    );
    final filterController = useTextEditingController();
    final filter = useState('');
    final scrollController = useScrollController();

    final pubkeys = [for (final agent in agents) agent.pubkey];
    final pubkeysKey = pubkeys.join(',');
    useEffect(() {
      if (pubkeys.isEmpty) return null;
      Future.microtask(() {
        ref.read(presenceCacheProvider.notifier).track(pubkeys);
        unawaited(ref.read(userCacheProvider.notifier).preload(pubkeys));
      });
      return null;
    }, [pubkeysKey]);

    useEffect(() {
      final reselection = tabReselection;
      if (reselection == null) return null;
      void scrollToTop() {
        if (!scrollController.hasClients) return;
        unawaited(
          scrollController.animateTo(
            0,
            duration: const Duration(milliseconds: 250),
            curve: Curves.easeOut,
          ),
        );
      }

      reselection.addListener(scrollToTop);
      return () => reselection.removeListener(scrollToTop);
    }, [tabReselection, scrollController]);

    String nameOf(MyAgent agent) {
      final name = users[agent.pubkey]?.displayName?.trim();
      if (name != null && name.isNotEmpty) return name;
      return agent.directoryName ?? shortPubkey(agent.pubkey);
    }

    final needle = filter.value.trim().toLowerCase();
    bool matches(String text) =>
        needle.isEmpty || text.toLowerCase().contains(needle);

    // One group per computer: this device first, then by computer name.
    final groups = <String, List<MyAgent>>{};
    for (final agent in agents) {
      if (matches(nameOf(agent))) (groups[agent.deviceId] ??= []).add(agent);
    }
    for (final list in groups.values) {
      list.sort(
        (a, b) => nameOf(a).toLowerCase().compareTo(nameOf(b).toLowerCase()),
      );
    }
    String computerOf(String deviceId) => myAgentComputerLabel(
      deviceNames[deviceId],
      current: currentDevice == deviceId,
    );
    final deviceIds = groups.keys.toList()
      ..sort((a, b) {
        if (a == currentDevice) return -1;
        if (b == currentDevice) return 1;
        final byName = computerOf(
          a,
        ).toLowerCase().compareTo(computerOf(b).toLowerCase());
        return byName != 0 ? byName : a.compareTo(b);
      });

    final agentsByPubkey = {for (final agent in agents) agent.pubkey: agent};
    String memberName(MyTeamMember member) {
      for (final pubkey in member.agentPubkeys) {
        final agent = agentsByPubkey[pubkey];
        if (agent != null) return nameOf(agent);
      }
      return member.name ?? 'Missing agent';
    }

    final teams = [
      for (final team in teamsAsync.value ?? const <MyTeam>[])
        if (matches(team.name) || team.members.map(memberName).any(matches))
          team,
    ];
    final presets = [
      for (final preset in presetsAsync.value ?? const <MyAgentPreset>[])
        if (matches(preset.name)) preset,
    ];

    final headerTitleStyle = context.textTheme.titleMedium?.copyWith(
      fontSize: 22,
      fontWeight: FontWeight.w600,
      color: navigationPrimaryForeground(context),
    );
    final topSectionHeight = frostedAppBarHeight(
      context,
      titleStyle: headerTitleStyle,
      bottomHeight: Grid.xxs,
    );
    Widget note(String text, {Key? key}) => Padding(
      key: key,
      padding: const EdgeInsets.symmetric(
        horizontal: Grid.gutter,
        vertical: Grid.xs,
      ),
      child: Text(
        text,
        style: context.textTheme.bodySmall?.copyWith(
          color: context.colors.onSurfaceVariant,
        ),
      ),
    );

    Future<void> refresh() async {
      ref.invalidate(agentDirectoryProvider);
      refreshMyAgentDefinitions(ref);
      if (pubkeys.isNotEmpty) {
        ref.read(presenceCacheProvider.notifier).track(pubkeys);
      }
      // Teams and presets are only watched (and fetched) on Presets.
      if (onPresets) {
        await ref.read(myTeamsProvider.future).catchError((_) => <MyTeam>[]);
      }
    }

    return FrostedScaffold(
      backgroundColor: context.colors.surface,
      appBar: FrostedAppBar(
        automaticallyImplyLeading: false,
        horizontalInset: Grid.gutter,
        showBottomDivider: true,
        bottomDividerOpacity: 0.07,
        title: Text('Agents', style: headerTitleStyle),
        titleStyle: headerTitleStyle,
        bottomHeight: Grid.xxs,
        bottom: const SizedBox.expand(),
      ),
      body: RefreshIndicator(
        edgeOffset: topSectionHeight,
        onRefresh: refresh,
        child: ListView(
          key: const ValueKey('agents-list'),
          controller: scrollController,
          physics: const AlwaysScrollableScrollPhysics(),
          padding: EdgeInsets.only(
            top: topSectionHeight,
            bottom: MediaQuery.paddingOf(context).bottom + Grid.xs,
          ),
          children: [
            Padding(
              padding: const EdgeInsets.fromLTRB(
                Grid.gutter,
                Grid.xxs,
                Grid.gutter,
                0,
              ),
              child: SizedBox(
                width: double.infinity,
                child: SegmentedButton<_AgentsTab>(
                  key: const Key('agents-tabs'),
                  segments: const [
                    ButtonSegment(
                      value: _AgentsTab.myAgents,
                      label: Text('My agents', key: Key('agents-tab-agents')),
                    ),
                    ButtonSegment(
                      value: _AgentsTab.presets,
                      label: Text('Presets', key: Key('agents-tab-presets')),
                    ),
                  ],
                  selected: {tab.value},
                  showSelectedIcon: false,
                  style: const ButtonStyle(
                    visualDensity: VisualDensity.compact,
                    tapTargetSize: MaterialTapTargetSize.shrinkWrap,
                  ),
                  onSelectionChanged: (selected) => tab.value = selected.single,
                ),
              ),
            ),
            AgentsFilterField(
              controller: filterController,
              hint: onPresets ? 'Find presets' : 'Find agents',
              onChanged: (value) => filter.value = value,
            ),
            if (onPresets)
              ..._presetsChildren(
                context,
                loading:
                    (presetsAsync.isLoading && !presetsAsync.hasValue) ||
                    (teamsAsync.isLoading && !teamsAsync.hasValue),
                presets: presets,
                teams: teams,
                memberName: memberName,
                filtering: needle.isNotEmpty,
                note: note,
              )
            else ...[
              if (agents.isEmpty && directoryLoading)
                const Padding(
                  padding: EdgeInsets.all(Grid.sm),
                  child: Center(child: KuzzLoadingIndicator()),
                )
              else if (agents.isEmpty)
                note(
                  'No agents yet. Agents run on your computers: create one in '
                  'the Kuzz app on your computer.',
                  key: const Key('agents-empty'),
                )
              else if (groups.isEmpty)
                note(
                  'No agents match that name.',
                  key: const Key('agents-no-match'),
                ),
              for (final deviceId in deviceIds) ...[
                _ComputerHeader(
                  key: ValueKey('agents-computer-$deviceId'),
                  name: computerOf(deviceId),
                  robot: resolveDeviceRobot(
                    deviceId,
                    override: robots[deviceId],
                  ),
                ),
                for (final agent in groups[deviceId]!)
                  MyAgentTile(agent: agent, name: nameOf(agent)),
              ],
            ],
          ],
        ),
      ),
    );
  }
}

enum _AgentsTab { myAgents, presets }

/// The Presets tab: a note that agents are made on a computer, the agent
/// presets with how many agents use each, then the team presets.
List<Widget> _presetsChildren(
  BuildContext context, {
  required bool loading,
  required List<MyAgentPreset> presets,
  required List<MyTeam> teams,
  required String Function(MyTeamMember) memberName,
  required bool filtering,
  required Widget Function(String text, {Key? key}) note,
}) => [
  Container(
    key: const Key('agents-presets-note'),
    margin: const EdgeInsets.fromLTRB(
      Grid.gutter,
      Grid.xxs,
      Grid.gutter,
      Grid.xxs,
    ),
    padding: const EdgeInsets.all(Grid.xs),
    decoration: BoxDecoration(
      color: context.colors.surfaceContainerHighest,
      borderRadius: BorderRadius.circular(Radii.md),
    ),
    child: Text(
      'Presets are templates. Create agents in the Kuzz app on your computer.',
      style: context.textTheme.bodySmall?.copyWith(
        color: context.colors.onSurfaceVariant,
      ),
    ),
  ),
  if (loading)
    const Padding(
      padding: EdgeInsets.all(Grid.sm),
      child: Center(child: KuzzLoadingIndicator()),
    )
  else if (presets.isEmpty && teams.isEmpty)
    note(
      filtering ? 'No presets match that name.' : 'No presets yet.',
      key: const Key('agents-presets-empty'),
    ),
  if (presets.isNotEmpty) ...[
    const AgentsSectionTitle('Agent presets'),
    for (final preset in presets)
      _PresetRow(key: ValueKey('agents-preset-${preset.id}'), preset: preset),
  ],
  if (teams.isNotEmpty) ...[
    const AgentsSectionTitle('Team presets'),
    for (final team in teams)
      _TeamRow(
        key: ValueKey('agents-team-${team.id}'),
        team: team,
        memberNames: [for (final m in team.members) memberName(m)],
      ),
  ],
];
