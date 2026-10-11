part of '../thread_goals_page.dart';

/// "Link to goal" from a message's actions: the thread goal picker, limited
/// by [messageGoalLinkScope] ([threadRootId] is the thread the message is
/// in, or null on the main timeline). The message id joins the goal's thread
/// links, so it becomes a thread link once someone replies.
Future<void> showMessageGoalLink({
  required BuildContext context,
  required WidgetRef ref,
  required String channelId,
  required String messageId,
  required String? threadRootId,
}) async {
  final messenger = ScaffoldMessenger.maybeOf(context);
  final GoalTree tree;
  try {
    tree = (await ref.read(goalTreeProvider(channelId).future)).tree;
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
  final links = _ThreadLinks(channelId: channelId, threadRootId: messageId);
  switch (choice) {
    case (goalId: final String goalId, createUnder: _):
      await links.link(context, ref, goalId);
    case (goalId: _, createUnder: final GoalRow parent):
      await links.createAndLink(context, ref, parent.node, parent.layer);
    default:
      await links.unlink(context, ref);
  }
}
