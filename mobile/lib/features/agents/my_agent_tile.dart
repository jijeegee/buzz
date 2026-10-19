import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/devices/agent_host_devices.dart';
import '../../shared/devices/device_robot.dart';
import '../../shared/devices/device_robot_icon.dart';
import '../../shared/profile/user_cache_provider.dart';
import '../../shared/theme/theme.dart';
import '../../shared/widgets/avatar_image.dart';
import '../profile/presence_cache_provider.dart';
import 'my_agent_actions.dart';
import 'my_agent_menu.dart';
import 'my_agent_status_dot.dart';
import 'my_agents.dart';

/// One of the owner's agents in Agents: avatar with a status light, the
/// robot of the computer it runs on before its name, its status (and, with
/// [showComputer], the computer) below, and the "⋮" for its actions.
/// Tapping it opens the DM with the agent; a long press or the "⋮" opens
/// the actions sheet.
class MyAgentTile extends ConsumerWidget {
  const MyAgentTile({
    super.key,
    required this.agent,
    required this.name,
    this.showComputer = false,
    this.indent = 0,
  });

  final MyAgent agent;
  final String name;
  final bool showComputer;
  final double indent;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final profile = ref.watch(
      userCacheProvider.select((cache) => cache[agent.pubkey]),
    );
    final status = myAgentStatus(
      ref.watch(presenceCacheProvider.select((p) => p[agent.pubkey])),
    );
    final robot = resolveDeviceRobot(
      agent.deviceId,
      override: ref.watch(deviceRobotOverridesProvider)[agent.deviceId],
    );
    final computer = showComputer
        ? myAgentComputerLabel(
            ref.watch(agentHostDeviceNamesProvider)[agent.deviceId],
            current:
                ref.watch(currentAccountDeviceIdProvider) == agent.deviceId,
          )
        : null;
    final initial = name.trim().isNotEmpty
        ? String.fromCharCode(name.trim().runes.first).toUpperCase()
        : '?';
    final mutedStyle = activityTimestampTextStyle.copyWith(
      color: context.colors.onSurfaceVariant,
    );
    return InkWell(
      key: ValueKey('my-agent-${agent.pubkey}'),
      onTap: () => unawaited(openMyAgentDm(context, ref, agent.pubkey, name)),
      onLongPress: () {
        unawaited(HapticFeedback.mediumImpact());
        unawaited(showMyAgentSheet(context, ref, agent: agent, name: name));
      },
      child: Padding(
        padding: EdgeInsets.fromLTRB(
          Grid.gutter + indent,
          Grid.half,
          Grid.xxs,
          Grid.half,
        ),
        child: Row(
          children: [
            Stack(
              clipBehavior: Clip.none,
              children: [
                AvatarImage(
                  imageUrl: profile?.avatarUrl,
                  radius: activityAvatarSize / 2,
                  backgroundColor: context.colors.primaryContainer,
                  fallback: Text(
                    initial,
                    style: TextStyle(
                      fontSize: 14,
                      fontWeight: FontWeight.w600,
                      color: context.colors.onPrimaryContainer,
                    ),
                  ),
                  isAgent: true,
                ),
                Positioned(
                  right: -1,
                  bottom: -1,
                  child: DecoratedBox(
                    decoration: BoxDecoration(
                      shape: BoxShape.circle,
                      border: Border.all(
                        color: context.colors.surface,
                        width: 2,
                      ),
                    ),
                    child: MyAgentStatusDot(
                      key: ValueKey('my-agent-dot-${agent.pubkey}'),
                      status: status,
                      size: 10,
                    ),
                  ),
                ),
              ],
            ),
            const SizedBox(width: messageAvatarContentGap),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Row(
                    children: [
                      DeviceRobotIcon(variant: robot, size: 14),
                      const SizedBox(width: Grid.quarter),
                      Flexible(
                        child: Text(
                          name,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: activityUsernameTextStyle.copyWith(
                            color: context.colors.onSurface,
                          ),
                        ),
                      ),
                    ],
                  ),
                  Text(
                    computer == null
                        ? myAgentStatusLabel(status)
                        : '${myAgentStatusLabel(status)} · $computer',
                    key: ValueKey('my-agent-status-${agent.pubkey}'),
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: mutedStyle,
                  ),
                ],
              ),
            ),
            MyAgentMenu(agent: agent, name: name),
          ],
        ),
      ),
    );
  }
}
