import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../shared/theme/theme.dart';
import '../../shared/widgets/modal_presentation.dart';
import 'goal_provider.dart';
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

Future<void> _run(BuildContext context, Future<void> Function() action) async {
  try {
    await action();
  } catch (error) {
    if (!context.mounted) return;
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(content: Text(error.toString().replaceFirst('Exception: ', ''))),
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
              final result = await _editGoal(context, title: 'Layer 1 goal');
              if (result == null || !context.mounted) return;
              await _run(
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
            onTap: () => _showGoalActions(context, ref, root, layer: 1),
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
          _GoalTile(
            key: ValueKey('goal-row-${row.node.id}'),
            layer: row.layer,
            node: row.node,
            progress: tree.progress(row.node.id),
            onTap: () =>
                _showGoalActions(context, ref, row.node, layer: row.layer),
            onToggleStatus: () => _run(
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
          onPressed: () => _addChild(context, ref, root.id, 2),
        ),
      ],
    );
  }

  Future<void> _addChild(
    BuildContext context,
    WidgetRef ref,
    String parentId,
    int layer,
  ) async {
    final result = await _editGoal(context, title: 'New layer $layer goal');
    if (result == null || !context.mounted) return;
    await _run(
      context,
      () => ref
          .read(goalActionsProvider)
          .apply(
            channelId,
            AddGoal(
              id: newGoalId(),
              parent: parentId,
              title: result.title,
              note: result.note,
            ),
          ),
    );
  }

  Future<void> _showGoalActions(
    BuildContext context,
    WidgetRef ref,
    GoalNode node, {
    required int layer,
  }) async {
    final actions = ref.read(goalActionsProvider);
    final subGoals = tree.subtreeIds(node.id).length - 1;
    final choice = await showBuzzModalBottomSheet<String>(
      context: context,
      title: node.title,
      showDragHandle: true,
      builder: (sheetContext) => SafeArea(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            ListTile(
              leading: const Icon(LucideIcons.pencil),
              title: const Text('Edit'),
              onTap: () => Navigator.pop(sheetContext, 'edit'),
            ),
            for (final status in GoalStatus.values)
              if (status != node.status)
                ListTile(
                  leading: const Icon(LucideIcons.circleCheck),
                  title: Text('Mark ${status.label.toLowerCase()}'),
                  onTap: () => Navigator.pop(sheetContext, status.wire),
                ),
            ListTile(
              leading: const Icon(LucideIcons.plus),
              title: Text('Add a layer ${layer + 1} goal'),
              onTap: () => Navigator.pop(sheetContext, 'add'),
            ),
            ListTile(
              leading: Icon(LucideIcons.trash2, color: context.colors.error),
              title: Text(
                subGoals > 0 ? 'Remove with $subGoals sub-goals' : 'Remove',
                style: TextStyle(color: context.colors.error),
              ),
              onTap: () => Navigator.pop(sheetContext, 'remove'),
            ),
          ],
        ),
      ),
    );
    if (choice == null || !context.mounted) return;
    switch (choice) {
      case 'edit':
        final result = await _editGoal(
          context,
          title: layer == 1 ? 'Layer 1 goal' : 'Goal',
          initialTitle: node.title,
          initialNote: node.note,
        );
        if (result == null || !context.mounted) return;
        await _run(
          context,
          () => actions.apply(
            channelId,
            layer == 1
                ? SetRootGoal(
                    id: node.id,
                    title: result.title,
                    note: result.note ?? '',
                  )
                : UpdateGoal(
                    id: node.id,
                    title: result.title,
                    note: result.note ?? '',
                  ),
          ),
        );
      case 'add':
        await _addChild(context, ref, node.id, layer + 1);
      case 'remove':
        final confirmed = await showDialog<bool>(
          context: context,
          builder: (dialogContext) => AlertDialog(
            title: const Text('Remove goal?'),
            content: Text(
              subGoals > 0
                  ? 'This also removes its $subGoals sub-goals. History keeps them.'
                  : 'History keeps it.',
            ),
            actions: [
              TextButton(
                onPressed: () => Navigator.pop(dialogContext, false),
                child: const Text('Cancel'),
              ),
              TextButton(
                onPressed: () => Navigator.pop(dialogContext, true),
                child: const Text('Remove'),
              ),
            ],
          ),
        );
        if (confirmed != true || !context.mounted) return;
        await _run(
          context,
          () => actions.apply(
            channelId,
            RemoveGoal(id: node.id, recursive: subGoals > 0),
          ),
        );
      default:
        await _run(
          context,
          () => actions.apply(
            channelId,
            UpdateGoal(id: node.id, status: GoalStatus.parse(choice)),
          ),
        );
    }
  }
}

class _GoalTile extends StatelessWidget {
  const _GoalTile({
    super.key,
    required this.layer,
    required this.node,
    required this.progress,
    required this.onTap,
    required this.onToggleStatus,
  });

  final int layer;
  final GoalNode node;
  final ({int done, int total}) progress;
  final VoidCallback onTap;
  final VoidCallback onToggleStatus;

  @override
  Widget build(BuildContext context) {
    final finished =
        node.status == GoalStatus.done || node.status == GoalStatus.dropped;
    return InkWell(
      onTap: onTap,
      child: Padding(
        padding: EdgeInsets.only(
          left: (layer - 2) * Grid.gutter,
          top: Grid.xxs,
          bottom: Grid.xxs,
        ),
        child: Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            IconButton(
              tooltip: node.status.label,
              visualDensity: VisualDensity.compact,
              onPressed: onToggleStatus,
              icon: Icon(switch (node.status) {
                GoalStatus.open => LucideIcons.circle,
                GoalStatus.inProgress => LucideIcons.circleDot,
                GoalStatus.done => LucideIcons.circleCheck,
                GoalStatus.dropped => LucideIcons.circleSlash,
              }, size: 20),
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
  }
}

typedef _GoalDraft = ({String title, String? note});

Future<_GoalDraft?> _editGoal(
  BuildContext context, {
  required String title,
  String initialTitle = '',
  String initialNote = '',
}) => showDialog<_GoalDraft>(
  context: context,
  builder: (_) => _GoalEditorDialog(
    heading: title,
    initialTitle: initialTitle,
    initialNote: initialNote,
  ),
);

class _GoalEditorDialog extends HookWidget {
  const _GoalEditorDialog({
    required this.heading,
    required this.initialTitle,
    required this.initialNote,
  });

  final String heading;
  final String initialTitle;
  final String initialNote;

  @override
  Widget build(BuildContext context) {
    final titleController = useTextEditingController(text: initialTitle);
    final noteController = useTextEditingController(text: initialNote);
    useListenable(titleController);
    final error = goalTitleError(titleController.text);
    return AlertDialog(
      title: Text(heading),
      content: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          TextField(
            key: const ValueKey('goal-editor-title'),
            controller: titleController,
            autofocus: true,
            maxLines: 1,
            decoration: InputDecoration(
              labelText: 'Goal (one sentence)',
              errorText: titleController.text.isEmpty ? null : error,
            ),
          ),
          TextField(
            key: const ValueKey('goal-editor-note'),
            controller: noteController,
            minLines: 2,
            maxLines: 5,
            decoration: const InputDecoration(
              labelText: 'Key information (optional)',
            ),
          ),
        ],
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.pop(context),
          child: const Text('Cancel'),
        ),
        FilledButton(
          key: const ValueKey('goal-editor-save'),
          onPressed: error != null
              ? null
              : () => Navigator.pop<_GoalDraft>(context, (
                  title: titleController.text.trim(),
                  note: noteController.text.trim().isEmpty
                      ? null
                      : noteController.text,
                )),
          child: const Text('Save'),
        ),
      ],
    );
  }
}
