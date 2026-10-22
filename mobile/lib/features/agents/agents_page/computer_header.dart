part of '../agents_page.dart';

/// The title over one computer's agents: its robot, then its name.
class _ComputerHeader extends StatelessWidget {
  const _ComputerHeader({super.key, required this.name, required this.robot});

  final String name;
  final DeviceRobotVariant? robot;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.fromLTRB(
      Grid.gutter,
      Grid.sm,
      Grid.gutter,
      Grid.half,
    ),
    child: Row(
      children: [
        DeviceRobotIcon(variant: robot, size: 16),
        const SizedBox(width: Grid.half),
        Flexible(
          child: Text(
            name,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: context.textTheme.labelLarge?.copyWith(
              color: context.colors.onSurfaceVariant,
              fontWeight: FontWeight.w700,
            ),
          ),
        ),
      ],
    ),
  );
}
