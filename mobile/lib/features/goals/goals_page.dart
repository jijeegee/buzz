import 'package:flutter/material.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../shared/l10n/l10n.dart';
import '../../shared/theme/theme.dart';
import 'goal_actions_sheet.dart';
import 'goal_editing.dart';
import 'goal_links.dart';
import 'goal_provider.dart';
import 'goal_thread_navigation.dart';
import 'goal_tile.dart';
import 'goal_tree.dart';

/// Above this many active goals under layer 1, the page says so.
const _topGoalsNotice = 7;

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

/// Layer 1 goal card, the outline of active goals, and the quiet and done
/// lists, read when opened.
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
      appBar: AppBar(title: Text(context.l10n.goalsPageTitle(channelName))),
      body: RefreshIndicator(
        onRefresh: () => ref.refresh(goalTreeProvider(channelId).future),
        child: headAsync.when(
          loading: () => const Center(child: CircularProgressIndicator()),
          error: (error, _) => ListView(
            padding: const EdgeInsets.all(Grid.gutter),
            children: [Text(context.l10n.goalsLoadError('$error'))],
          ),
          data: (board) => _GoalsBody(channelId: channelId, board: board),
        ),
      ),
    );
  }
}

class _GoalsBody extends ConsumerWidget {
  const _GoalsBody({required this.channelId, required this.board});

  final String channelId;
  final GoalBoard board;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final tree = board.tree;
    final root = tree.root;
    final actions = ref.read(goalActionsProvider);
    if (root == null) {
      return ListView(
        padding: const EdgeInsets.all(Grid.gutter),
        children: [
          Text(
            context.l10n.goalsEmptyIntro,
            style: context.textTheme.bodyMedium,
          ),
          const SizedBox(height: Grid.twelve),
          FilledButton.icon(
            key: const ValueKey('goals-set-root'),
            icon: const Icon(LucideIcons.target, size: 18),
            label: Text(context.l10n.goalsSetGoal),
            onPressed: () async {
              final result = await editGoal(
                context,
                title: context.l10n.goalsLayer1Goal,
              );
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

    final linkKinds = ref
        .watch(goalLinkKindsProvider(goalLinkKindsKey(channelId, tree)))
        .value;
    final progress = root.progress;
    return ListView(
      padding: const EdgeInsets.all(Grid.gutter),
      children: [
        if (root.openChildren > _topGoalsNotice) ...[
          Text(
            context.l10n.goalsTopGoals(root.openChildren),
            key: const ValueKey('goals-top-notice'),
            style: context.textTheme.bodySmall,
          ),
          const SizedBox(height: Grid.xs),
        ],
        Card(
          key: const ValueKey('goals-root-card'),
          child: InkWell(
            onTap: () => showGoalActions(
              context,
              ref,
              channelId: channelId,
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
                      Text(
                        context.l10n.goalsLayer1Goal,
                        style: context.textTheme.labelSmall,
                      ),
                    ],
                  ),
                  const SizedBox(height: Grid.xs),
                  Text(root.title, style: context.textTheme.titleMedium),
                  if (root.note.isNotEmpty)
                    GoalNote(goalId: root.id, note: root.note),
                  if (progress.total > 0) ...[
                    const SizedBox(height: Grid.twelve),
                    LinearProgressIndicator(
                      value: progress.done / progress.total,
                    ),
                    const SizedBox(height: Grid.xxs),
                    Text(
                      context.l10n.goalsProgress(progress.done, progress.total),
                      style: context.textTheme.bodySmall,
                    ),
                  ],
                ],
              ),
            ),
          ),
        ),
        const SizedBox(height: Grid.twelve),
        GoalOutline(
          tree: tree,
          tops: [
            for (final node in tree.children(root.id)) (layer: 2, node: node),
          ],
          rowBuilder: (row, fold) => _GoalRow(
            channelId: channelId,
            row: row,
            fold: fold,
            indent: row.layer - 2,
            linkKinds: linkKinds,
          ),
        ),
        TextButton.icon(
          key: const ValueKey('goals-add-layer2'),
          icon: const Icon(LucideIcons.plus, size: 18),
          label: Text(context.l10n.goalsAddLayerGoal(2)),
          onPressed: () => addChildGoal(
            context,
            ref,
            channelId: channelId,
            parentId: root.id,
            layer: 2,
          ),
        ),
        _GoalListSection(
          channelId: channelId,
          view: GoalView.inactive,
          count: board.counts.inactive,
          label: context.l10n.goalsSectionQuiet,
        ),
        _GoalListSection(
          channelId: channelId,
          view: GoalView.done,
          count: board.counts.done,
          label: context.l10n.goalsSectionDone,
        ),
      ],
    );
  }
}

/// One editable goal of this page.
class _GoalRow extends ConsumerWidget {
  const _GoalRow({
    required this.channelId,
    required this.row,
    required this.fold,
    required this.indent,
    required this.linkKinds,
  });

  final String channelId;
  final GoalRow row;
  final GoalFold fold;
  final int indent;
  final Map<String, GoalLinkInfo>? linkKinds;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final node = row.node;
    return GoalTile(
      key: ValueKey('goal-row-${node.id}'),
      layer: row.layer,
      indent: indent,
      node: node,
      fold: fold,
      onTap: () => showGoalActions(
        context,
        ref,
        channelId: channelId,
        node: node,
        layer: row.layer,
      ),
      onToggleStatus: () => setGoalStatus(
        context,
        ref,
        channelId: channelId,
        node: node,
        status: node.status.next,
      ),
      onOpenLink: (target) => openGoalLink(
        context,
        ref,
        channelId: channelId,
        target: target,
        replaceCurrent: false,
      ),
      linkKinds: linkKinds,
    );
  }
}

/// A list that folds away and reads its goals when opened.
class _GoalListSection extends StatefulWidget {
  const _GoalListSection({
    required this.channelId,
    required this.view,
    required this.count,
    required this.label,
  });

  final String channelId;
  final GoalView view;
  final int count;
  final String label;

  @override
  State<_GoalListSection> createState() => _GoalListSectionState();
}

class _GoalListSectionState extends State<_GoalListSection> {
  bool _open = false;

  @override
  Widget build(BuildContext context) {
    if (widget.count == 0) return const SizedBox.shrink();
    final view = widget.view.name;
    return Column(
      key: ValueKey('goals-section-$view'),
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        InkWell(
          key: ValueKey('goals-section-toggle-$view'),
          onTap: () => setState(() => _open = !_open),
          child: Padding(
            padding: const EdgeInsets.symmetric(vertical: Grid.xs),
            child: Row(
              children: [
                Icon(
                  _open ? LucideIcons.chevronDown : LucideIcons.chevronRight,
                  size: 16,
                ),
                const SizedBox(width: Grid.xxs),
                Expanded(
                  child: Text(
                    widget.label,
                    style: context.textTheme.labelMedium,
                  ),
                ),
                Text('${widget.count}', style: context.textTheme.bodySmall),
              ],
            ),
          ),
        ),
        if (_open)
          _GoalListRows(
            listKey: (channelId: widget.channelId, view: widget.view),
          ),
      ],
    );
  }
}

class _GoalListRows extends ConsumerWidget {
  const _GoalListRows({required this.listKey});

  final GoalListKey listKey;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final list = ref.watch(goalListProvider(listKey));
    final read = list.value;
    if (read == null) {
      if (list.hasError) {
        return Text(context.l10n.goalsLoadError('${list.error}'));
      }
      return const Padding(
        padding: EdgeInsets.all(Grid.xs),
        child: Center(child: CircularProgressIndicator()),
      );
    }
    final channelId = listKey.channelId;
    final tree = read.tree;
    final linkKinds = ref
        .watch(goalLinkKindsProvider(goalLinkKindsKey(channelId, tree)))
        .value;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (listKey.view == GoalView.inactive)
          Align(
            alignment: AlignmentDirectional.centerStart,
            child: TextButton(
              key: const ValueKey('goals-drop-quiet'),
              onPressed: () => _dropQuiet(context, ref),
              child: Text(context.l10n.goalsDropAll),
            ),
          ),
        for (final top in tree.listTops())
          _listTop(tree, top, linkKinds, context),
        if (read.next != null)
          Align(
            alignment: AlignmentDirectional.centerStart,
            child: TextButton(
              key: const ValueKey('goals-show-more'),
              onPressed: () => runGoalAction(
                context,
                ref.read(goalListProvider(listKey).notifier).loadMore,
              ),
              child: Text(context.l10n.goalsShowMore),
            ),
          ),
      ],
    );
  }

  Widget _listTop(
    GoalTree tree,
    GoalNode top,
    Map<String, GoalLinkInfo>? linkKinds,
    BuildContext context,
  ) {
    final path = tree.path(top.id);
    final above = path.skip(1).take(path.length - 2);
    return Column(
      key: ValueKey('goals-list-top-${top.id}'),
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (above.isNotEmpty)
          Text(
            above.map((node) => node.title).join(' › '),
            style: context.textTheme.bodySmall,
          ),
        GoalOutline(
          tree: tree,
          tops: [(layer: path.length, node: top)],
          rowBuilder: (row, fold) => _GoalRow(
            channelId: listKey.channelId,
            row: row,
            fold: fold,
            indent: row.layer - path.length,
            linkKinds: linkKinds,
          ),
        ),
      ],
    );
  }

  Future<void> _dropQuiet(BuildContext context, WidgetRef ref) async {
    final l10n = context.l10n;
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (dialogContext) => AlertDialog(
        content: Text(l10n.goalsDropAllConfirm),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(dialogContext, false),
            child: Text(l10n.goalsCancel),
          ),
          TextButton(
            key: const ValueKey('goals-drop-quiet-confirm'),
            onPressed: () => Navigator.pop(dialogContext, true),
            child: Text(l10n.goalsDropAll),
          ),
        ],
      ),
    );
    if (confirmed != true || !context.mounted) return;
    await runGoalAction(
      context,
      () => ref
          .read(goalActionsProvider)
          .apply(listKey.channelId, const DropQuietGoals()),
    );
  }
}
