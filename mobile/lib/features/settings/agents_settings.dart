import 'dart:async';

import 'package:flutter/material.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../shared/theme/grid.dart';
import '../../shared/widgets/app_list.dart';
import '../../shared/widgets/app_list_card.dart';
import '../channels/mentions/thread_agent_audience.dart';

/// Settings → Agents, matching desktop's Agents → Conversations switch.
class AgentsSettings extends ConsumerWidget {
  const AgentsSettings({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final enabled = ref.watch(keepMentionedAgentsPinnedProvider);
    void setEnabled(bool value) => unawaited(
      ref.read(keepMentionedAgentsPinnedProvider.notifier).setEnabled(value),
    );
    return AppListCard(
      key: const ValueKey('settings-agents'),
      label: 'Agents',
      verticalPadding: Grid.twelve,
      children: [
        AppListRow(
          key: const ValueKey('settings-automatic-agent-mentions'),
          icon: LucideIcons.atSign,
          title: 'Automatically mention agents',
          subtitle: 'Address selected agents in thread replies',
          trailing: Switch.adaptive(value: enabled, onChanged: setEnabled),
          onTap: () => setEnabled(!enabled),
        ),
      ],
    );
  }
}
