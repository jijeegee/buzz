part of '../thread_goals_page.dart';

/// The linked goal: its path, a card with status and progress, its sub-goal
/// tree with an add button, and relink / unlink actions.
class _LinkedGoalBody extends ConsumerWidget {
  const _LinkedGoalBody({
    required this.channelId,
    required this.tree,
    required this.goal,
    required this.links,
  });

  final String channelId;
  final GoalTree tree;
  final GoalNode goal;
  final _ThreadLinks links;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final path = tree.path(goal.id);
    final layer = path.length;
    final progress = tree.progress(goal.id);
    final rows = <GoalRow>[];
    void walk(GoalNode node, int depth) {
      for (final child in tree.children(node.id)) {
        rows.add((layer: depth, node: child));
        if (depth < maxGoalDepth) walk(child, depth + 1);
      }
    }

    walk(goal, layer + 1);
    final actions = ref.read(goalActionsProvider);
    void toggle(GoalNode node) => runGoalAction(
      context,
      () => actions.apply(
        channelId,
        UpdateGoal(id: node.id, status: node.status.next),
      ),
    );

    return ListView(
      padding: const EdgeInsets.all(Grid.gutter),
      children: [
        if (path.length > 1)
          Text(
            path.take(path.length - 1).map((node) => node.title).join(' › '),
            key: const ValueKey('thread-goal-ancestors'),
            style: context.textTheme.bodySmall,
          ),
        const SizedBox(height: Grid.xxs),
        Card(
          key: const ValueKey('thread-goal-card'),
          child: InkWell(
            onTap: () => showGoalActions(
              context,
              ref,
              channelId: channelId,
              tree: tree,
              node: goal,
              layer: layer,
            ),
            child: Padding(
              padding: const EdgeInsets.all(Grid.twelve),
              child: Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  IconButton(
                    key: ValueKey('goal-status-${goal.id}'),
                    tooltip: goal.status.label,
                    onPressed: () => toggle(goal),
                    icon: Icon(goalStatusIcon(goal.status), size: 22),
                  ),
                  const SizedBox(width: Grid.half),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          'Layer $layer goal',
                          style: context.textTheme.labelSmall,
                        ),
                        Text(goal.title, style: context.textTheme.titleMedium),
                        if (goal.note.isNotEmpty)
                          Text(goal.note, style: context.textTheme.bodySmall),
                        if (progress.total > 0) ...[
                          const SizedBox(height: Grid.xxs),
                          LinearProgressIndicator(
                            value: progress.done / progress.total,
                          ),
                          const SizedBox(height: Grid.half),
                          Text(
                            '${progress.done} of ${progress.total} sub-goals done',
                            style: context.textTheme.bodySmall,
                          ),
                        ],
                      ],
                    ),
                  ),
                ],
              ),
            ),
          ),
        ),
        const SizedBox(height: Grid.xxs),
        for (final row in rows)
          GoalTile(
            key: ValueKey('goal-row-${row.node.id}'),
            layer: row.layer,
            indent: row.layer - layer - 1,
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
            onToggleStatus: () => toggle(row.node),
          ),
        TextButton.icon(
          key: const ValueKey('thread-goal-add'),
          icon: const Icon(LucideIcons.plus, size: 18),
          label: Text('Add a layer ${layer + 1} goal'),
          onPressed: () => addChildGoal(
            context,
            ref,
            channelId: channelId,
            parentId: goal.id,
            layer: layer + 1,
          ),
        ),
        const Divider(),
        TextButton.icon(
          key: const ValueKey('thread-goal-change'),
          icon: const Icon(LucideIcons.link, size: 18),
          label: const Text('Link to a different goal'),
          onPressed: () => _relink(context, ref),
        ),
        TextButton.icon(
          key: const ValueKey('thread-goal-unlink'),
          icon: const Icon(LucideIcons.unlink, size: 18),
          label: const Text('Unlink this thread'),
          onPressed: () => links.unlink(context, ref),
        ),
      ],
    );
  }

  Future<void> _relink(BuildContext context, WidgetRef ref) async {
    final choice = await showBuzzModalBottomSheet<({GoalRow row, bool create})>(
      context: context,
      title: 'Link to a goal',
      isScrollControlled: true,
      builder: (sheetContext) => SafeArea(
        child: SingleChildScrollView(
          padding: const EdgeInsets.symmetric(horizontal: Grid.gutter),
          child: _GoalLinkList(
            tree: tree,
            linkedGoalId: goal.id,
            onLink: (id) => Navigator.pop(sheetContext, (
              row: (layer: 0, node: tree.node(id) ?? goal),
              create: false,
            )),
            onCreateUnder: (parent, layer) => Navigator.pop(sheetContext, (
              row: (layer: layer, node: parent),
              create: true,
            )),
          ),
        ),
      ),
    );
    if (choice == null || !context.mounted) return;
    if (choice.create) {
      await links.createAndLink(
        context,
        ref,
        choice.row.node,
        choice.row.layer,
      );
    } else {
      await links.link(context, ref, choice.row.node.id);
    }
  }
}
