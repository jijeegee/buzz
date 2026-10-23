part of '../agents_page.dart';

/// One of the owner's teams on My agents: its name, its agents' names and a
/// status light per agent, like desktop's team card. Tapping it opens the
/// team.
class _TeamRow extends ConsumerWidget {
  const _TeamRow({super.key, required this.team, required this.agentName});

  final MyAgentTeam team;

  /// The agent's name; `null` when it is no longer one of the owner's.
  final String? Function(String pubkey) agentName;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final presence = ref.watch(presenceCacheProvider);
    final names = [for (final pubkey in team.agentPubkeys) ?agentName(pubkey)];
    final removed = team.agentPubkeys.length - names.length;
    final subtitle = [
      if (names.isEmpty && removed == 0)
        context.l10n.agentsTeamNoAgents
      else
        ...names,
      if (removed > 0) context.l10n.agentsTeamRemoved(removed),
    ].join(' · ');
    return InkWell(
      onTap: () => unawaited(
        Navigator.of(context).push(
          MaterialPageRoute<void>(builder: (_) => AgentTeamPage(team: team)),
        ),
      ),
      child: Padding(
        padding: const EdgeInsets.symmetric(
          horizontal: Grid.gutter,
          vertical: Grid.half,
        ),
        child: Row(
          children: [
            Container(
              width: activityAvatarSize,
              height: activityAvatarSize,
              decoration: BoxDecoration(
                color: context.colors.surfaceContainerHighest,
                borderRadius: BorderRadius.circular(Radii.md),
              ),
              child: Icon(
                LucideIcons.users,
                size: 20,
                color: context.colors.onSurfaceVariant,
              ),
            ),
            const SizedBox(width: messageAvatarContentGap),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    team.name,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: activityUsernameTextStyle.copyWith(
                      color: context.colors.onSurface,
                    ),
                  ),
                  Text(
                    subtitle,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: activityTimestampTextStyle.copyWith(
                      color: context.colors.onSurfaceVariant,
                    ),
                  ),
                ],
              ),
            ),
            for (final pubkey in team.agentPubkeys.take(6))
              if (agentName(pubkey) != null)
                Padding(
                  padding: const EdgeInsetsDirectional.only(
                    start: Grid.quarter,
                  ),
                  child: MyAgentStatusDot(
                    key: ValueKey('agents-team-dot-${team.id}-$pubkey'),
                    status: myAgentStatus(presence[pubkey]),
                  ),
                ),
            const SizedBox(width: Grid.quarter),
            Icon(
              LucideIcons.chevronRightDir,
              size: 18,
              color: context.colors.onSurfaceVariant,
            ),
          ],
        ),
      ),
    );
  }
}

/// One team preset on Presets: its name and its agent presets' names. View
/// only: team presets are set up on a computer in the Kuzz app there.
class _TeamPresetRow extends StatelessWidget {
  const _TeamPresetRow({
    super.key,
    required this.team,
    required this.memberNames,
  });

  final MyTeamPreset team;
  final List<String> memberNames;

  @override
  Widget build(BuildContext context) {
    final subtitle = memberNames.isEmpty
        ? context.l10n.agentsTeamNoPresets
        : memberNames.join(' · ');
    return Padding(
      padding: const EdgeInsets.symmetric(
        horizontal: Grid.gutter,
        vertical: Grid.half,
      ),
      child: Row(
        children: [
          Container(
            width: activityAvatarSize,
            height: activityAvatarSize,
            decoration: BoxDecoration(
              color: context.colors.surfaceContainerHighest,
              borderRadius: BorderRadius.circular(Radii.md),
            ),
            child: Icon(
              LucideIcons.users,
              size: 20,
              color: context.colors.onSurfaceVariant,
            ),
          ),
          const SizedBox(width: messageAvatarContentGap),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  team.name,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: activityUsernameTextStyle.copyWith(
                    color: context.colors.onSurface,
                  ),
                ),
                Text(
                  subtitle,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: activityTimestampTextStyle.copyWith(
                    color: context.colors.onSurfaceVariant,
                  ),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}
