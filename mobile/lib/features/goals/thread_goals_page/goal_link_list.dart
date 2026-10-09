part of '../thread_goals_page.dart';

/// Every goal of the conversation: tap one to link the thread to it, or +
/// to add a new goal under it and link to that.
class _GoalLinkList extends StatelessWidget {
  const _GoalLinkList({
    required this.tree,
    required this.linkedGoalId,
    required this.onLink,
    required this.onCreateUnder,
  });

  final GoalTree tree;
  final String? linkedGoalId;
  final void Function(String goalId) onLink;
  final void Function(GoalNode parent, int layer) onCreateUnder;

  @override
  Widget build(BuildContext context) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        for (final row in tree.outline())
          Row(
            key: ValueKey('thread-goal-option-${row.node.id}'),
            children: [
              Expanded(
                child: InkWell(
                  key: ValueKey('thread-goal-link-${row.node.id}'),
                  onTap: () => onLink(row.node.id),
                  child: Padding(
                    padding: EdgeInsets.only(
                      left: (row.layer - 1) * Grid.xs,
                      top: Grid.xxs,
                      bottom: Grid.xxs,
                    ),
                    child: Text(
                      'L${row.layer}  ${row.node.title}',
                      style: context.textTheme.bodyMedium?.copyWith(
                        fontWeight: row.node.id == linkedGoalId
                            ? FontWeight.w600
                            : null,
                      ),
                    ),
                  ),
                ),
              ),
              IconButton(
                key: ValueKey('thread-goal-create-under-${row.node.id}'),
                tooltip: 'Add a goal under this and link the thread',
                visualDensity: VisualDensity.compact,
                icon: const Icon(LucideIcons.plus, size: 18),
                onPressed: () => onCreateUnder(row.node, row.layer),
              ),
            ],
          ),
      ],
    );
  }
}
