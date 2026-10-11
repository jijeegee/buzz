import 'package:flutter/material.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../shared/theme/theme.dart';
import 'goal_tree.dart';

/// One goal in an outline: status toggle, `L<layer>` title, progress, and
/// its linked threads. Without [onTap] and [onToggleStatus] it is read-only;
/// [muted] fades it. Thread links open through [onOpenThread], even when
/// read-only; [currentThreadId]'s link reads "This thread" and stays put.
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
    this.onOpenThread,
    this.currentThreadId,
  });

  final int layer;

  /// Nesting depth inside the list it is shown in (0 = flush).
  final int indent;
  final GoalNode node;
  final ({int done, int total}) progress;
  final VoidCallback? onTap;
  final VoidCallback? onToggleStatus;
  final bool muted;
  final ValueChanged<String>? onOpenThread;
  final String? currentThreadId;

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
                    if (progress.total > 0)
                      Text(
                        '${progress.done}/${progress.total}',
                        style: context.textTheme.bodySmall,
                      ),
                    if (node.threads.isNotEmpty)
                      Wrap(
                        spacing: Grid.xs,
                        children: [
                          for (final thread in node.threads)
                            _ThreadLink(
                              key: ValueKey('goal-thread-$thread'),
                              isCurrent: thread == currentThreadId,
                              onTap:
                                  onOpenThread == null ||
                                      thread == currentThreadId
                                  ? null
                                  : () => onOpenThread!(thread),
                            ),
                        ],
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

class _ThreadLink extends StatelessWidget {
  const _ThreadLink({super.key, required this.isCurrent, required this.onTap});

  final bool isCurrent;
  final VoidCallback? onTap;

  @override
  Widget build(BuildContext context) {
    final color = onTap == null
        ? context.colors.onSurfaceVariant
        : context.colors.primary;
    return InkWell(
      onTap: onTap,
      child: Padding(
        padding: const EdgeInsets.symmetric(vertical: Grid.half),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(LucideIcons.messagesSquare, size: 14, color: color),
            const SizedBox(width: Grid.half),
            Text(
              isCurrent ? 'This thread' : 'Thread',
              style: context.textTheme.bodySmall?.copyWith(color: color),
            ),
          ],
        ),
      ),
    );
  }
}

IconData goalStatusIcon(GoalStatus status) => switch (status) {
  GoalStatus.open => LucideIcons.circle,
  GoalStatus.inProgress => LucideIcons.circleDot,
  GoalStatus.done => LucideIcons.circleCheck,
  GoalStatus.dropped => LucideIcons.circleSlash,
};
