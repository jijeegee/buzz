import 'package:flutter/material.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../shared/l10n/l10n.dart';
import '../../shared/theme/theme.dart';
import 'goal_links.dart';
import 'goal_tree.dart';

/// Whether a goal's sub-goals show; [toggle] is null when it has none.
typedef GoalFold = ({bool open, VoidCallback? toggle});

/// [tops] and the goals below them, leaving out what folded goals hide. Plan
/// goals and [unfolded] start open, thread goals closed. Rows come from
/// [rowBuilder].
class GoalOutline extends StatefulWidget {
  const GoalOutline({
    super.key,
    required this.tree,
    required this.tops,
    required this.rowBuilder,
    this.unfolded = const {},
  });

  final GoalTree tree;
  final List<GoalRow> tops;
  final Widget Function(GoalRow row, GoalFold fold) rowBuilder;
  final Set<String> unfolded;

  @override
  State<GoalOutline> createState() => _GoalOutlineState();
}

class _GoalOutlineState extends State<GoalOutline> {
  /// Goals folded or unfolded by hand, away from how they start.
  final _flipped = <String>{};

  bool _isOpen(String id) {
    final start = widget.unfolded.contains(id) || widget.tree.isPlanGoal(id);
    return _flipped.contains(id) ? !start : start;
  }

  void _flip(String id) => setState(() {
    if (!_flipped.remove(id)) _flipped.add(id);
  });

  @override
  Widget build(BuildContext context) {
    final rows = <Widget>[];
    void walk(GoalRow row) {
      final id = row.node.id;
      final children = widget.tree.children(id);
      final open = _isOpen(id);
      rows.add(
        widget.rowBuilder(row, (
          open: open,
          toggle: children.isEmpty ? null : () => _flip(id),
        )),
      );
      if (!open || row.layer >= maxGoalDepth) return;
      for (final child in children) {
        walk((layer: row.layer + 1, node: child));
      }
    }

    widget.tops.forEach(walk);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: rows,
    );
  }
}

/// One goal in an outline: fold button, status toggle, `L<layer>` title,
/// progress, and its linked threads and messages. Without [onTap] and
/// [onToggleStatus] it is read-only; [muted] fades it. Links open through
/// [onOpenLink], even when read-only; links are icons, and
/// [currentThreadId]'s stays put. [linkKinds] tells threads from messages;
/// unknown links show as threads.
class GoalTile extends StatelessWidget {
  const GoalTile({
    super.key,
    required this.layer,
    required this.indent,
    required this.node,
    required this.fold,
    required this.onTap,
    required this.onToggleStatus,
    this.muted = false,
    this.onOpenLink,
    this.currentThreadId,
    this.linkKinds,
  });

  final int layer;

  /// Nesting depth inside the list it is shown in (0 = flush).
  final int indent;
  final GoalNode node;
  final GoalFold fold;
  final VoidCallback? onTap;
  final VoidCallback? onToggleStatus;
  final bool muted;
  final ValueChanged<GoalLinkTarget>? onOpenLink;
  final String? currentThreadId;
  final Map<String, GoalLinkInfo>? linkKinds;

  @override
  Widget build(BuildContext context) {
    final finished =
        node.status == GoalStatus.done || node.status == GoalStatus.dropped;
    final progress = node.progress;
    final toggleFold = fold.toggle;
    final tile = InkWell(
      onTap: onTap,
      child: Padding(
        padding: EdgeInsetsDirectional.only(
          start: indent * Grid.gutter,
          top: Grid.xxs,
          bottom: Grid.xxs,
        ),
        child: Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            SizedBox(
              width: Grid.sm,
              child: toggleFold == null
                  ? null
                  : IconButton(
                      key: ValueKey('goal-fold-${node.id}'),
                      tooltip: fold.open
                          ? context.l10n.goalsCollapse
                          : context.l10n.goalsExpand,
                      padding: EdgeInsets.zero,
                      visualDensity: VisualDensity.compact,
                      onPressed: toggleFold,
                      icon: Icon(
                        fold.open
                            ? LucideIcons.chevronDown
                            : LucideIcons.chevronRight,
                        size: 16,
                      ),
                    ),
            ),
            IconButton(
              key: ValueKey('goal-status-${node.id}'),
              tooltip: node.status.label(context.l10n),
              visualDensity: VisualDensity.compact,
              onPressed: onToggleStatus,
              icon: Icon(goalStatusIcon(node.status), size: 20),
            ),
            Expanded(
              child: Padding(
                padding: const EdgeInsets.only(top: Grid.xs),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      context.l10n.goalsLayerTitle(layer, node.title),
                      style: context.textTheme.bodyMedium?.copyWith(
                        decoration: finished
                            ? TextDecoration.lineThrough
                            : null,
                      ),
                    ),
                    if (progress.total > 0)
                      Text(
                        '${progress.done}/${progress.total}',
                        style: context.textTheme.bodySmall,
                      ),
                    if (node.links.isNotEmpty)
                      Wrap(
                        spacing: Grid.xs,
                        children: [
                          for (final thread in node.links)
                            _link(context, thread),
                        ],
                      ),
                  ],
                ),
              ),
            ),
          ],
        ),
      ),
    );
    return muted ? Opacity(opacity: 0.5, child: tile) : tile;
  }

  Widget _link(BuildContext context, String id) {
    final isCurrent = id == currentThreadId;
    final (:label, :target) = goalLinkView(id, linkKinds?[id]);
    final open = onOpenLink;
    return _ThreadLink(
      key: ValueKey('goal-thread-$id'),
      label: isCurrent ? context.l10n.goalsThisThread : label,
      onTap: isCurrent || target == null || open == null
          ? null
          : () => open(target),
    );
  }
}

class _ThreadLink extends StatelessWidget {
  const _ThreadLink({super.key, required this.label, required this.onTap});

  /// What the link is, for its tooltip and screen readers; not shown.
  final String label;
  final VoidCallback? onTap;

  @override
  Widget build(BuildContext context) {
    final color = onTap == null
        ? context.colors.onSurfaceVariant
        : context.colors.primary;
    return Tooltip(
      message: switch (label) {
        'Message' => context.l10n.goalsGoToMessage,
        'Thread' => context.l10n.goalsOpenThread,
        _ => label,
      },
      child: InkWell(
        onTap: onTap,
        child: Padding(
          padding: const EdgeInsets.all(Grid.half),
          child: Icon(
            label == 'Message'
                ? LucideIcons.messageSquare
                : LucideIcons.messagesSquare,
            size: 16,
            color: color,
          ),
        ),
      ),
    );
  }
}

/// A goal's note, hidden behind a note button until pressed.
class GoalNote extends StatefulWidget {
  const GoalNote({super.key, required this.goalId, required this.note});

  final String goalId;
  final String note;

  @override
  State<GoalNote> createState() => _GoalNoteState();
}

class _GoalNoteState extends State<GoalNote> {
  bool _open = false;

  @override
  Widget build(BuildContext context) {
    final color = _open
        ? context.colors.primary
        : context.colors.onSurfaceVariant;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Tooltip(
          message: _open
              ? context.l10n.goalsHideNote
              : context.l10n.goalsShowNote,
          child: InkWell(
            key: ValueKey('goal-note-toggle-${widget.goalId}'),
            onTap: () => setState(() => _open = !_open),
            child: Padding(
              padding: const EdgeInsets.all(Grid.half),
              child: Icon(LucideIcons.stickyNote, size: 16, color: color),
            ),
          ),
        ),
        if (_open) Text(widget.note, style: context.textTheme.bodySmall),
      ],
    );
  }
}

IconData goalStatusIcon(GoalStatus status) => switch (status) {
  GoalStatus.open => LucideIcons.circle,
  GoalStatus.inProgress => LucideIcons.circleDot,
  GoalStatus.done => LucideIcons.circleCheck,
  GoalStatus.dropped => LucideIcons.circleSlash,
};
