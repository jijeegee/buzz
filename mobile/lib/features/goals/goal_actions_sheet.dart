import 'package:flutter/material.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../shared/theme/theme.dart';
import '../../shared/widgets/modal_presentation.dart';
import 'goal_editing.dart';
import 'goal_provider.dart';
import 'goal_tree.dart';

/// Asks for a new goal under [parentId] (on [layer]) and adds it.
Future<void> addChildGoal(
  BuildContext context,
  WidgetRef ref, {
  required String channelId,
  required String parentId,
  required int layer,
}) async {
  final result = await editGoal(context, title: 'New layer $layer goal');
  if (result == null || !context.mounted) return;
  await runGoalAction(
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

/// Edit, set status, add a sub-goal, or remove [node] (on [layer]).
Future<void> showGoalActions(
  BuildContext context,
  WidgetRef ref, {
  required String channelId,
  required GoalTree tree,
  required GoalNode node,
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
      final result = await editGoal(
        context,
        title: layer == 1 ? 'Layer 1 goal' : 'Goal',
        initialTitle: node.title,
        initialNote: node.note,
      );
      if (result == null || !context.mounted) return;
      await runGoalAction(
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
      await addChildGoal(
        context,
        ref,
        channelId: channelId,
        parentId: node.id,
        layer: layer + 1,
      );
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
      await runGoalAction(
        context,
        () => actions.apply(
          channelId,
          RemoveGoal(id: node.id, recursive: subGoals > 0),
        ),
      );
    default:
      await runGoalAction(
        context,
        () => actions.apply(
          channelId,
          UpdateGoal(id: node.id, status: GoalStatus.parse(choice)),
        ),
      );
  }
}
