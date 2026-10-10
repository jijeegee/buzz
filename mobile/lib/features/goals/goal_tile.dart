import 'package:flutter/material.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../shared/theme/theme.dart';
import 'goal_tree.dart';

/// One goal in an outline: status toggle, `L<layer>` title, and progress.
/// Without [onTap] and [onToggleStatus] it is read-only; [muted] fades it.
class GoalTile extends StatelessWidget {
  const GoalTile({
    super.key,
    required this.layer,
    required this.indent,
    required this.node,
    required this.progress,
    required this.onTap,
    required this.onToggleStatus,
    this.muted = false,
  });

  final int layer;

  /// Nesting depth inside the list it is shown in (0 = flush).
  final int indent;
  final GoalNode node;
  final ({int done, int total}) progress;
  final VoidCallback? onTap;
  final VoidCallback? onToggleStatus;
  final bool muted;

  @override
  Widget build(BuildContext context) {
    final finished =
        node.status == GoalStatus.done || node.status == GoalStatus.dropped;
    final tile = InkWell(
      onTap: onTap,
      child: Padding(
        padding: EdgeInsets.only(
          left: indent * Grid.gutter,
          top: Grid.xxs,
          bottom: Grid.xxs,
        ),
        child: Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            IconButton(
              key: ValueKey('goal-status-${node.id}'),
              tooltip: node.status.label,
              visualDensity: VisualDensity.compact,
              onPressed: onToggleStatus,
              icon: Icon(goalStatusIcon(node.status), size: 20),
            ),
            Expanded(
              child: Padding(
                padding: const EdgeInsets.only(top: Grid.xs),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      'L$layer  ${node.title}',
                      style: context.textTheme.bodyMedium?.copyWith(
                        decoration: finished
                            ? TextDecoration.lineThrough
                            : null,
                      ),
                    ),
                    if (progress.total > 0 || node.threads.isNotEmpty)
                      Text(
                        [
                          if (progress.total > 0)
                            '${progress.done}/${progress.total}',
                          if (node.threads.isNotEmpty)
                            '${node.threads.length} thread${node.threads.length == 1 ? '' : 's'}',
                        ].join(' · '),
                        style: context.textTheme.bodySmall,
                      ),
                  ],
                ),
              ),
            ),
          ],
        ),
      ),
    );
    return muted ? Opacity(opacity: 0.5, child: tile) : tile;
  }
}

IconData goalStatusIcon(GoalStatus status) => switch (status) {
  GoalStatus.open => LucideIcons.circle,
  GoalStatus.inProgress => LucideIcons.circleDot,
  GoalStatus.done => LucideIcons.circleCheck,
  GoalStatus.dropped => LucideIcons.circleSlash,
};
