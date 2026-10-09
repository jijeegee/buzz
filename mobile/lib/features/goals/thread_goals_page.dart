import 'package:flutter/material.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../shared/theme/theme.dart';
import '../../shared/widgets/modal_presentation.dart';
import 'goal_actions_sheet.dart';
import 'goal_editing.dart';
import 'goal_provider.dart';
import 'goal_tile.dart';
import 'goal_tree.dart';

part 'thread_goals_page/goal_link_list.dart';
part 'thread_goals_page/linked_goal_body.dart';

/// Opens the goal a thread works on.
Future<void> showThreadGoalsPage({
  required BuildContext context,
  required String channelId,
  required String threadRootId,
}) => Navigator.of(context).push<void>(
  MaterialPageRoute<void>(
    builder: (_) =>
        ThreadGoalsPage(channelId: channelId, threadRootId: threadRootId),
  ),
);

/// A thread's goal: the linked goal with its sub-goal tree (add sub-goals,
/// change status, relink or unlink), or a picker that links the thread to a
/// goal or adds a new goal for it and links it in one revision.
class ThreadGoalsPage extends ConsumerWidget {
  const ThreadGoalsPage({
    super.key,
    required this.channelId,
    required this.threadRootId,
  });

  final String channelId;
  final String threadRootId;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final headAsync = ref.watch(goalTreeProvider(channelId));
    return Scaffold(
      appBar: AppBar(title: const Text('Thread goal')),
      body: RefreshIndicator(
        onRefresh: () => ref.refresh(goalTreeProvider(channelId).future),
        child: headAsync.when(
          loading: () => const Center(child: CircularProgressIndicator()),
          error: (error, _) => ListView(
            padding: const EdgeInsets.all(Grid.gutter),
            children: [Text('Could not load goals: $error')],
          ),
          data: (head) => _body(context, ref, head.tree),
        ),
      ),
    );
  }

  Widget _body(BuildContext context, WidgetRef ref, GoalTree tree) {
    if (tree.root == null) {
      return ListView(
        padding: const EdgeInsets.all(Grid.gutter),
        children: [
          Text(
            'This conversation has no goals yet. Set its goal from the '
            'channel first.',
            style: context.textTheme.bodyMedium,
          ),
        ],
      );
    }
    final goal = tree.forThread(threadRootId);
    final links = _ThreadLinks(
      channelId: channelId,
      threadRootId: threadRootId,
    );
    if (goal == null) {
      return ListView(
        padding: const EdgeInsets.all(Grid.gutter),
        children: [
          Text(
            'Link this thread to the goal it works on, or add a new goal for '
            'it with +.',
            style: context.textTheme.bodyMedium,
          ),
          const SizedBox(height: Grid.twelve),
          _GoalLinkList(
            tree: tree,
            linkedGoalId: null,
            onLink: (id) => links.link(context, ref, id),
            onCreateUnder: (parent, layer) =>
                links.createAndLink(context, ref, parent, layer),
          ),
        ],
      );
    }
    return _LinkedGoalBody(
      channelId: channelId,
      tree: tree,
      goal: goal,
      links: links,
    );
  }
}

/// Link edits for one thread.
class _ThreadLinks {
  const _ThreadLinks({required this.channelId, required this.threadRootId});

  final String channelId;
  final String threadRootId;

  Future<void> link(BuildContext context, WidgetRef ref, String goalId) =>
      runGoalAction(
        context,
        () => ref
            .read(goalActionsProvider)
            .apply(channelId, LinkThread(id: goalId, thread: threadRootId)),
      );

  Future<void> unlink(BuildContext context, WidgetRef ref) => runGoalAction(
    context,
    () => ref
        .read(goalActionsProvider)
        .apply(channelId, UnlinkThread(thread: threadRootId)),
  );

  /// Adds a goal under [parent] (on [layer]) and links this thread to it,
  /// as one revision.
  Future<void> createAndLink(
    BuildContext context,
    WidgetRef ref,
    GoalNode parent,
    int layer,
  ) async {
    final result = await editGoal(
      context,
      title: 'New layer ${layer + 1} goal',
      saveLabel: 'Add and link',
    );
    if (result == null || !context.mounted) return;
    final id = newGoalId();
    await runGoalAction(
      context,
      () => ref.read(goalActionsProvider).applyAll(channelId, [
        AddGoal(
          id: id,
          parent: parent.id,
          title: result.title,
          note: result.note,
        ),
        LinkThread(id: id, thread: threadRootId),
      ]),
    );
  }
}
