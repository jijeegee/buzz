import 'package:flutter/material.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../shared/theme/theme.dart';
import 'goal_actions_sheet.dart';
import 'goal_editing.dart';
import 'goal_provider.dart';
import 'goal_tile.dart';
import 'goal_tree.dart';

/// Opens the goals of a channel or DM.
Future<void> showGoalsPage({
  required BuildContext context,
  required String channelId,
  required String channelName,
}) => Navigator.of(context).push<void>(
  MaterialPageRoute<void>(
    builder: (_) => GoalsPage(channelId: channelId, channelName: channelName),
  ),
);

/// Layer 1 goal card plus an indented outline of every sub-goal.
class GoalsPage extends ConsumerWidget {
  const GoalsPage({
    super.key,
    required this.channelId,
    required this.channelName,
  });

  final String channelId;
  final String channelName;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final headAsync = ref.watch(goalTreeProvider(channelId));
    return Scaffold(
      appBar: AppBar(title: Text('Goals · $channelName')),
      body: RefreshIndicator(
        onRefresh: () => ref.refresh(goalTreeProvider(channelId).future),
        child: headAsync.when(
          loading: () => const Center(child: CircularProgressIndicator()),
          error: (error, _) => ListView(
            padding: const EdgeInsets.all(Grid.gutter),
            children: [Text('Could not load goals: $error')],
          ),
          data: (head) => _GoalsBody(channelId: channelId, tree: head.tree),
        ),
      ),
    );
  }
}

class _GoalsBody extends ConsumerWidget {
  const _GoalsBody({required this.channelId, required this.tree});

  final String channelId;
  final GoalTree tree;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final root = tree.root;
    final actions = ref.read(goalActionsProvider);
    if (root == null) {
      return ListView(
        padding: const EdgeInsets.all(Grid.gutter),
        children: [
          Text(
            'Give this conversation one top goal. Split it into smaller goals, '
            'and agents working here will see where their work fits.',
            style: context.textTheme.bodyMedium,
          ),
          const SizedBox(height: Grid.twelve),
          FilledButton.icon(
            key: const ValueKey('goals-set-root'),
            icon: const Icon(LucideIcons.target, size: 18),
            label: const Text('Set the goal'),
            onPressed: () async {
              final result = await editGoal(context, title: 'Layer 1 goal');
              if (result == null || !context.mounted) return;
              await runGoalAction(
                context,
                () => actions.apply(
                  channelId,
                  SetRootGoal(
                    id: newGoalId(),
                    title: result.title,
                    note: result.note,
                  ),
                ),
              );
            },
          ),
        ],
      );
    }

    final rows = tree.outline().skip(1).toList();
    final progress = tree.progress(root.id);
    return ListView(
      padding: const EdgeInsets.all(Grid.gutter),
      children: [
        Card(
          key: const ValueKey('goals-root-card'),
          child: InkWell(
            onTap: () => showGoalActions(
              context,
              ref,
              channelId: channelId,
              tree: tree,
              node: root,
              layer: 1,
            ),
            child: Padding(
              padding: const EdgeInsets.all(Grid.gutter),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Row(
                    children: [
                      Icon(
                        LucideIcons.target,
                        size: 18,
                        color: context.colors.primary,
                      ),
                      const SizedBox(width: Grid.xs),
                      Text('Layer 1 goal', style: context.textTheme.labelSmall),
                    ],
                  ),
                  const SizedBox(height: Grid.xs),
                  Text(root.title, style: context.textTheme.titleMedium),
                  if (root.note.isNotEmpty) ...[
                    const SizedBox(height: Grid.xs),
                    Text(root.note, style: context.textTheme.bodySmall),
                  ],
                  if (progress.total > 0) ...[
                    const SizedBox(height: Grid.twelve),
                    LinearProgressIndicator(
                      value: progress.done / progress.total,
                    ),
                    const SizedBox(height: Grid.xxs),
                    Text(
                      '${progress.done} of ${progress.total} goals done',
                      style: context.textTheme.bodySmall,
                    ),
                  ],
                ],
              ),
            ),
          ),
        ),
        const SizedBox(height: Grid.twelve),
        for (final row in rows)
          GoalTile(
            key: ValueKey('goal-row-${row.node.id}'),
            layer: row.layer,
            indent: row.layer - 2,
            node: row.node,
            progress: tree.progress(row.node.id),
            onTap: () => showGoalActions(
              context,
              ref,
              channelId: channelId,
              tree: tree,
              node: row.node,
              layer: row.layer,
            ),
            onToggleStatus: () => runGoalAction(
              context,
              () => actions.apply(
                channelId,
                UpdateGoal(id: row.node.id, status: row.node.status.next),
              ),
            ),
          ),
        TextButton.icon(
          key: const ValueKey('goals-add-layer2'),
          icon: const Icon(LucideIcons.plus, size: 18),
          label: const Text('Add a layer 2 goal'),
          onPressed: () => addChildGoal(
            context,
            ref,
            channelId: channelId,
            parentId: root.id,
            layer: 2,
          ),
        ),
      ],
    );
  }
}
