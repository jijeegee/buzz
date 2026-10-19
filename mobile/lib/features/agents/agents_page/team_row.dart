part of '../agents_page.dart';

/// One agent team: its name and its members' names, like desktop's team
/// row. Tapping it opens the team.
class _TeamRow extends StatelessWidget {
  const _TeamRow({super.key, required this.team, required this.memberNames});

  final MyTeam team;
  final List<String> memberNames;

  @override
  Widget build(BuildContext context) {
    final subtitle = memberNames.isEmpty
        ? 'No presets'
        : memberNames.join(' · ');
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
            Icon(
              LucideIcons.chevronRight,
              size: 18,
              color: context.colors.onSurfaceVariant,
            ),
          ],
        ),
      ),
    );
  }
}
