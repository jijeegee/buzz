import 'package:flutter/material.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../shared/theme/theme.dart';
import 'goal_links.dart';
import 'goal_tree.dart';

/// One goal in an outline: status toggle, `L<layer>` title, progress, and
/// its linked threads and messages. Without [onTap] and [onToggleStatus] it
/// is read-only; [muted] fades it. Links open through [onOpenLink], even when
/// read-only; [currentThreadId]'s link reads "This thread" and stays put.
/// [linkKinds] tells threads from messages; unknown links show as threads.
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
    this.onOpenLink,
    this.currentThreadId,
    this.linkKinds,
  });

  final int layer;

  /// Nesting depth inside the list it is shown in (0 = flush).
  final int indent;
  final GoalNode node;
  final ({int done, int total}) progress;
  final VoidCallback? onTap;
  final VoidCallback? onToggleStatus;
  final bool muted;
  final ValueChanged<GoalLinkTarget>? onOpenLink;
  final String? currentThreadId;
  final Map<String, GoalLinkInfo>? linkKinds;

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
                          for (final thread in node.threads) _link(thread),
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

  Widget _link(String id) {
    final isCurrent = id == currentThreadId;
    final (:label, :target) = goalLinkView(id, linkKinds?[id]);
    final open = onOpenLink;
    return _ThreadLink(
      key: ValueKey('goal-thread-$id'),
      label: isCurrent ? 'This thread' : label,
      onTap: isCurrent || target == null || open == null
          ? null
          : () => open(target),
    );
  }
}

class _ThreadLink extends StatelessWidget {
  const _ThreadLink({super.key, required this.label, required this.onTap});

  final String label;
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
            Icon(
              label == 'Message'
                  ? LucideIcons.messageSquare
                  : LucideIcons.messagesSquare,
              size: 14,
              color: color,
            ),
            const SizedBox(width: Grid.half),
            Text(
              label,
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
