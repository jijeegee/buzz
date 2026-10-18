import 'dart:async';

import 'package:flutter/material.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../profile/presence_cache_provider.dart';
import 'my_agent_actions.dart';
import 'my_agents.dart';

/// The "⋮" menu on an agent: Start, Stop and Edit, like desktop's card menu
/// for an agent on another computer.
class MyAgentMenu extends ConsumerWidget {
  const MyAgentMenu({super.key, required this.agent, required this.name});

  final MyAgent agent;
  final String name;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final status = myAgentStatus(
      ref.watch(presenceCacheProvider.select((p) => p[agent.pubkey])),
    );
    final busy = ref.watch(
      myAgentCommandsProvider.select((c) => c.containsKey(agent.pubkey)),
    );
    return PopupMenuButton<_MenuChoice>(
      key: ValueKey('my-agent-menu-${agent.pubkey}'),
      icon: const Icon(LucideIcons.ellipsisVertical, size: 18),
      tooltip: 'Agent actions',
      onSelected: (choice) {
        switch (choice) {
          case _MenuChoice.start:
            unawaited(startMyAgent(context, ref, agent, name));
          case _MenuChoice.stop:
            unawaited(stopMyAgent(context, ref, agent, name));
          case _MenuChoice.edit:
            openMyAgentSettings(context, agent.pubkey, name);
        }
      },
      itemBuilder: (menuContext) {
        final canStart = !busy && myAgentCanStart(agent, status);
        final canStop = !busy && myAgentCanStop(agent, status);
        // The app's popup theme draws every label in one colour, so an item
        // that cannot be pressed would look pressable; dim it here.
        final dimmed = TextStyle(
          color: Theme.of(
            menuContext,
          ).colorScheme.onSurface.withValues(alpha: 0.38),
        );
        return [
          PopupMenuItem(
            key: ValueKey('my-agent-menu-start-${agent.pubkey}'),
            value: _MenuChoice.start,
            enabled: canStart,
            child: Text('Start', style: canStart ? null : dimmed),
          ),
          PopupMenuItem(
            key: ValueKey('my-agent-menu-stop-${agent.pubkey}'),
            value: _MenuChoice.stop,
            enabled: canStop,
            child: Text('Stop', style: canStop ? null : dimmed),
          ),
          PopupMenuItem(
            key: ValueKey('my-agent-menu-edit-${agent.pubkey}'),
            value: _MenuChoice.edit,
            child: const Text('Edit'),
          ),
        ];
      },
    );
  }
}

enum _MenuChoice { start, stop, edit }
