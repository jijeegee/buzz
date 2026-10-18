import 'dart:async';

import 'package:flutter/material.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/devices/agent_host_devices.dart';
import '../../shared/devices/device_robot.dart';
import '../../shared/devices/device_robot_icon.dart';
import '../../shared/theme/theme.dart';
import '../profile/presence_cache_provider.dart';
import 'my_agent_actions.dart';
import 'my_agent_status_dot.dart';
import 'my_agents.dart';

/// Status and Start / Stop for one of the owner's agents, shown at the top
/// of its settings. Nothing when [agentPubkey] is not one of their agents.
class MyAgentStatusCard extends ConsumerWidget {
  const MyAgentStatusCard({
    super.key,
    required this.agentPubkey,
    required this.name,
  });

  final String agentPubkey;
  final String name;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final pubkey = agentPubkey.toLowerCase();
    final agent = ref
        .watch(myAgentsProvider)
        .where((a) => a.pubkey == pubkey)
        .firstOrNull;
    if (agent == null) return const SizedBox.shrink();
    final status = myAgentStatus(
      ref.watch(presenceCacheProvider.select((p) => p[pubkey])),
    );
    final command = ref.watch(myAgentCommandsProvider.select((c) => c[pubkey]));
    final deviceName = ref.watch(agentHostDeviceNamesProvider)[agent.deviceId];
    final current = ref.watch(currentAccountDeviceIdProvider) == agent.deviceId;
    final robot = resolveDeviceRobot(
      agent.deviceId,
      override: ref.watch(deviceRobotOverridesProvider)[agent.deviceId],
    );
    final Widget? action = myAgentCanStop(agent, status)
        ? OutlinedButton(
            key: ValueKey('my-agent-stop-$pubkey'),
            onPressed: command == null
                ? () => unawaited(stopMyAgent(context, ref, agent, name))
                : null,
            child: Text(command == MyAgentCommand.stop ? 'Stopping…' : 'Stop'),
          )
        : myAgentCanStart(agent, status)
        ? FilledButton(
            key: ValueKey('my-agent-start-$pubkey'),
            onPressed: command == null
                ? () => unawaited(startMyAgent(context, ref, agent, name))
                : null,
            child: Text(
              command == MyAgentCommand.start ? 'Starting…' : 'Start',
            ),
          )
        : null;
    return Padding(
      key: const Key('my-agent-status-card'),
      padding: const EdgeInsets.only(bottom: Grid.xs),
      child: Row(
        children: [
          MyAgentStatusDot(status: status),
          const SizedBox(width: Grid.half),
          Text(myAgentStatusLabel(status), style: context.textTheme.bodyMedium),
          const SizedBox(width: Grid.xxs),
          DeviceRobotIcon(variant: robot, size: 16),
          const SizedBox(width: Grid.quarter),
          Expanded(
            child: Text(
              myAgentComputerLabel(deviceName, current: current),
              overflow: TextOverflow.ellipsis,
              style: context.textTheme.bodySmall?.copyWith(
                color: context.colors.onSurfaceVariant,
              ),
            ),
          ),
          ?action,
        ],
      ),
    );
  }
}
