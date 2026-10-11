import 'package:flutter/material.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../shared/theme/theme.dart';
import 'goal_provider.dart';
import 'thread_goals_page.dart';

/// Thread app bar goals button, the same one the channel app bar shows.
/// Tapping opens [ThreadGoalsPage]: the whole tree, with this thread's goal
/// branch editable, or the list to link one.
class ThreadGoalsButton extends ConsumerWidget {
  const ThreadGoalsButton({
    super.key,
    required this.channelId,
    required this.threadRootId,
  });

  final String channelId;
  final String threadRootId;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final root = ref
        .watch(goalTreeProvider(channelId))
        .asData
        ?.value
        .tree
        .root;
    return IconButton(
      key: const ValueKey('thread-goals-button'),
      color: context.colors.primary,
      tooltip: root?.title ?? 'Goals',
      onPressed: () => showThreadGoalsPage(
        context: context,
        channelId: channelId,
        threadRootId: threadRootId,
      ),
      icon: Icon(
        root == null ? LucideIcons.target : LucideIcons.crosshair,
        size: 22,
      ),
    );
  }
}
