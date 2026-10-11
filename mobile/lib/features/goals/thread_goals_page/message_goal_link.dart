part of '../thread_goals_page.dart';

/// "Link to goal" from a message's actions: the thread goal picker, limited
/// by [messageGoalLinkScope] ([threadRootId] is the thread the message is
/// in, or null on the main timeline). The message id joins the goal's thread
/// links, so it becomes a thread link once someone replies.
///
/// Pass a [context] that outlives the message row, such as its navigator's:
/// a scrolled-away row must not drop the pick.
Future<void> showMessageGoalLink({
  required BuildContext context,
  required String channelId,
  required String messageId,
  required String? threadRootId,
}) async {
  final messenger = ScaffoldMessenger.maybeOf(context);
  final container = ProviderScope.containerOf(context, listen: false);
  final GoalTree tree;
  try {
    tree = (await container.read(goalTreeProvider(channelId).future)).tree;
  } catch (error) {
    messenger?.showSnackBar(
      SnackBar(content: Text('Could not load goals: $error')),
    );
    return;
  }
  if (!context.mounted) return;
  if (tree.root == null) {
    messenger?.showSnackBar(
      const SnackBar(content: Text('This conversation has no goals yet.')),
    );
    return;
  }
  final linked = tree.forThread(messageId);
  final choice =
      await showBuzzModalBottomSheet<({String? goalId, GoalRow? createUnder})>(
        context: context,
        title: 'Link to goal',
        isScrollControlled: true,
        builder: (sheetContext) => SafeArea(
          child: SingleChildScrollView(
            padding: const EdgeInsets.symmetric(horizontal: Grid.gutter),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                _GoalLinkList(
                  tree: tree,
                  linkedGoalId: linked?.id,
                  scope: messageGoalLinkScope(tree, threadRootId),
                  subject: 'message',
                  onLink: (id) => Navigator.pop(sheetContext, (
                    goalId: id,
                    createUnder: null,
                  )),
                  onCreateUnder: (parent, layer) => Navigator.pop(
                    sheetContext,
                    (goalId: null, createUnder: (layer: layer, node: parent)),
                  ),
                ),
                if (linked != null)
                  TextButton.icon(
                    key: const ValueKey('message-goal-unlink'),
                    icon: const Icon(LucideIcons.unlink, size: 18),
                    label: const Text('Unlink from goal'),
                    onPressed: () => Navigator.pop(sheetContext, (
                      goalId: null,
                      createUnder: null,
                    )),
                  ),
              ],
            ),
          ),
        ),
      );
  if (choice == null || !context.mounted) return;
  final actions = container.read(goalActionsProvider);
  switch (choice) {
    case (goalId: final String goalId, createUnder: _):
      await runGoalAction(
        context,
        () =>
            actions.apply(channelId, LinkThread(id: goalId, thread: messageId)),
      );
    case (goalId: _, createUnder: final GoalRow parent):
      final result = await editGoal(
        context,
        title: 'New layer ${parent.layer + 1} goal',
        saveLabel: 'Add and link',
      );
      if (result == null || !context.mounted) return;
      final id = newGoalId();
      await runGoalAction(
        context,
        () => actions.applyAll(channelId, [
          AddGoal(
            id: id,
            parent: parent.node.id,
            title: result.title,
            note: result.note,
          ),
          LinkThread(id: id, thread: messageId),
        ]),
      );
    default:
      await runGoalAction(
        context,
        () => actions.apply(channelId, UnlinkThread(thread: messageId)),
      );
  }
}
