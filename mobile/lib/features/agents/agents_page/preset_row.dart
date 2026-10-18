part of '../agents_page.dart';

/// One agent preset: its letter, name, then its description and how many
/// agents use it. Presets are templates with no status; agents are made from
/// them in the Kuzz app on a computer.
class _PresetRow extends StatelessWidget {
  const _PresetRow({super.key, required this.preset});

  final MyAgentPreset preset;

  @override
  Widget build(BuildContext context) {
    final subtitle = [
      ?preset.description,
      presetUsageLabel(preset.agentCount),
    ].join(' · ');
    return Padding(
      padding: const EdgeInsets.symmetric(
        horizontal: Grid.gutter,
        vertical: Grid.half,
      ),
      child: Row(
        children: [
          CircleAvatar(
            radius: activityAvatarSize / 2,
            backgroundColor: context.colors.surfaceContainerHighest,
            child: Text(
              preset.name.characters.first.toUpperCase(),
              style: context.textTheme.titleSmall?.copyWith(
                color: context.colors.onSurfaceVariant,
              ),
            ),
          ),
          const SizedBox(width: messageAvatarContentGap),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  preset.name,
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
