import 'dart:async';

import 'package:flutter/material.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../../shared/theme/grid.dart';
import '../../shared/widgets/app_list.dart';
import '../../shared/widgets/app_list_card.dart';
import '../agents/my_agents_page.dart';
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
          key: const ValueKey('settings-my-agents'),
          icon: LucideIcons.bot,
          title: 'My agents',
          subtitle: 'Where your agents run: start, stop and edit them',
          trailing: const Icon(LucideIcons.chevronRight, size: 18),
          onTap: () => Navigator.of(
            context,
          ).push(MaterialPageRoute<void>(builder: (_) => const MyAgentsPage())),
        ),
        AppListRow(
          key: const ValueKey('settings-automatic-agent-mentions'),
          icon: LucideIcons.atSign,
          title: 'Automatically mention agents',
          subtitle: 'Address selected agents in channel and thread replies',
          trailing: Switch.adaptive(value: enabled, onChanged: setEnabled),
          onTap: () => setEnabled(!enabled),
        ),
      ],
    );
  }
}
