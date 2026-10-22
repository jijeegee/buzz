import 'package:flutter/material.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/profile/user_cache_provider.dart';
import '../../shared/theme/theme.dart';
import '../../shared/utils/string_utils.dart';
import 'agent_row.dart';
import 'my_agent_tile.dart';
import 'my_agents.dart';
import 'my_teams.dart';

/// One of the owner's teams: its description and its agents, each with its
/// status, computer and Start / Stop / Edit menu. Agents can be on
/// different computers. Teams are made and added to channels in the Kuzz app
/// on a computer.
class AgentTeamPage extends ConsumerWidget {
  const AgentTeamPage({super.key, required this.team});

  final MyAgentTeam team;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final agents = {
      for (final agent in ref.watch(myAgentsProvider)) agent.pubkey: agent,
    };
    final users = ref.watch(userCacheProvider);
    String nameOf(MyAgent agent) {
      final name = users[agent.pubkey]?.displayName?.trim();
      if (name != null && name.isNotEmpty) return name;
      return agent.directoryName ?? shortPubkey(agent.pubkey);
    }

    final removed = team.agentPubkeys.where((p) => !agents.containsKey(p));
    final muted = context.textTheme.bodySmall?.copyWith(
      color: context.colors.onSurfaceVariant,
    );
    return Scaffold(
      appBar: AppBar(title: Text(team.name)),
      body: SafeArea(
        child: ListView(
          key: const ValueKey('agent-team-page'),
          padding: const EdgeInsets.symmetric(vertical: Grid.xs),
          children: [
            if (team.description case final description?)
              Padding(
                padding: const EdgeInsets.symmetric(
                  horizontal: Grid.gutter,
                  vertical: Grid.xxs,
                ),
                child: Text(description, style: context.textTheme.bodyMedium),
              ),
            const AgentsSectionTitle('Agents'),
            if (team.agentPubkeys.isEmpty)
              Padding(
                padding: const EdgeInsets.symmetric(horizontal: Grid.gutter),
                child: Text('This team has no agents yet.', style: muted),
              ),
            for (final pubkey in team.agentPubkeys)
              if (agents[pubkey] case final agent?)
                MyAgentTile(
                  agent: agent,
                  name: nameOf(agent),
                  showComputer: true,
                ),
            if (removed.isNotEmpty)
              ListTile(
                key: const ValueKey('agent-team-removed'),
                contentPadding: const EdgeInsets.symmetric(
                  horizontal: Grid.gutter,
                ),
                title: Text(
                  removed.length == 1
                      ? '1 agent was removed'
                      : '${removed.length} agents were removed',
                ),
                subtitle: Text(
                  'No longer on any of your computers',
                  style: muted,
                ),
              ),
            Padding(
              padding: const EdgeInsets.fromLTRB(
                Grid.gutter,
                Grid.sm,
                Grid.gutter,
                0,
              ),
              child: Text(
                'Change teams and add them to groups in the Kuzz app on '
                'your computer.',
                style: muted,
              ),
            ),
          ],
        ),
      ),
    );
  }
}
