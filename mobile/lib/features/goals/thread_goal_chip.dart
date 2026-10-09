import 'package:flutter/material.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../shared/theme/theme.dart';
import 'goal_provider.dart';
import 'goal_tree.dart';
import 'thread_goals_page.dart';

/// Compact thread-head chip: the goal path this thread works on (below the
/// layer 1 goal, which the channel already shows) with sub-goal progress, or
/// an invitation to link one. Tapping opens [ThreadGoalsPage]. Nothing shows
/// when the conversation has no goals.
class ThreadGoalChip extends ConsumerWidget {
  const ThreadGoalChip({
    super.key,
    required this.channelId,
    required this.threadRootId,
  });

  final String channelId;
  final String threadRootId;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final tree = ref.watch(goalTreeProvider(channelId)).asData?.value.tree;
    if (tree == null || tree.root == null) return const SizedBox.shrink();
    final goal = tree.forThread(threadRootId);
    final path = goal == null ? const <GoalNode>[] : tree.path(goal.id);
    final shown = path.length > 1 ? path.skip(1) : path;
    final progress = goal == null ? null : tree.progress(goal.id);
    final label = goal == null
        ? 'Link this thread to a goal'
        : shown.map((node) => node.title).join(' › ');
    final progressLabel = progress != null && progress.total > 0
        ? '${progress.done}/${progress.total}'
        : null;
    return Padding(
      padding: const EdgeInsets.only(bottom: Grid.xxs),
      child: Align(
        alignment: Alignment.centerLeft,
        child: Semantics(
          button: true,
          label: goal == null
              ? label
              : 'Thread goal: $label${progressLabel == null ? '' : ', $progressLabel sub-goals done'}',
          excludeSemantics: true,
          child: Material(
            color: context.colors.surfaceContainerHighest,
            shape: const StadiumBorder(),
            child: InkWell(
              key: const ValueKey('thread-goal-chip'),
              customBorder: const StadiumBorder(),
              onTap: () => showThreadGoalsPage(
                context: context,
                channelId: channelId,
                threadRootId: threadRootId,
              ),
              child: Padding(
                padding: const EdgeInsets.symmetric(
                  horizontal: Grid.twelve,
                  vertical: Grid.half,
                ),
                child: Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Icon(
                      goal == null ? LucideIcons.link : LucideIcons.target,
                      size: 14,
                      color: context.colors.primary,
                    ),
                    const SizedBox(width: Grid.half),
                    Flexible(
                      child: Text(
                        label,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: context.textTheme.labelMedium?.copyWith(
                          color: goal == null
                              ? context.colors.onSurfaceVariant
                              : context.colors.onSurface,
                        ),
                      ),
                    ),
                    if (progressLabel != null) ...[
                      const SizedBox(width: Grid.half),
                      Text(
                        progressLabel,
                        key: const ValueKey('thread-goal-chip-progress'),
                        style: context.textTheme.labelSmall?.copyWith(
                          color: context.colors.onSurfaceVariant,
                        ),
                      ),
                    ],
                  ],
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}
