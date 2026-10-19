import 'package:flutter/material.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/profile/user_cache_provider.dart';
import '../../shared/theme/theme.dart';
import '../../shared/utils/string_utils.dart';
import 'agent_row.dart';
import 'my_agent_tile.dart';
import 'my_agents.dart';
import 'my_teams.dart';

/// One of the owner's agent teams: its description and its agents, each
/// with its status, computer and Start / Stop / Edit menu. Teams are made
/// and added to channels in the Kuzz app on a computer.
class AgentTeamPage extends ConsumerWidget {
  const AgentTeamPage({super.key, required this.team});

  final MyTeam team;

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
            if (team.members.isEmpty)
              Padding(
                padding: const EdgeInsets.symmetric(horizontal: Grid.gutter),
                child: Text('This team has no agents yet.', style: muted),
              ),
            for (final member in team.members)
              if (member.agentPubkeys.where(agents.containsKey).isEmpty)
                ListTile(
                  key: ValueKey('agent-team-member-${member.personaD}'),
                  contentPadding: const EdgeInsets.symmetric(
                    horizontal: Grid.gutter,
                  ),
                  title: Text(member.name ?? 'Missing agent'),
                  subtitle: Text(
                    member.name == null
                        ? 'Removed from your agents'
                        : 'Not on any of your computers',
                    style: muted,
                  ),
                )
              else
                for (final pubkey in member.agentPubkeys)
                  if (agents[pubkey] case final agent?)
                    MyAgentTile(
                      agent: agent,
                      name: nameOf(agent),
                      showComputer: true,
                    ),
            Padding(
              padding: const EdgeInsets.fromLTRB(
                Grid.gutter,
                Grid.sm,
                Grid.gutter,
                0,
              ),
              child: Text(
                'Change teams and add them to channels in the Kuzz app on '
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
