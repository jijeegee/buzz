import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/devices/agent_host_devices.dart';
import '../../shared/devices/device_robot.dart';
import '../../shared/devices/device_robot_icon.dart';
import '../../shared/profile/user_cache_provider.dart';
import '../../shared/profile/user_profile.dart';
import '../../shared/theme/theme.dart';
import '../../shared/widgets/app_list.dart';
import '../../shared/widgets/app_list_card.dart';
import '../../shared/widgets/modal_presentation.dart';
import '../channels/send_message_provider.dart';
import '../channels/small_avatar.dart';
import '../profile/presence_cache_provider.dart';
import 'agent_settings_page.dart';
import 'my_agents.dart';

/// The signed-in user's agents on their computers: where each runs, whether
/// it is online, Start and Stop, and (tap a row) its settings. Stop and Start
/// send the owner's `!shutdown` / `!start` command in a channel the agent is
/// in, like desktop's remote stop; the Kuzz app on the agent's computer acts
/// on it, so that app must be open.
class MyAgentsPage extends HookConsumerWidget {
  const MyAgentsPage({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final agents = ref.watch(myAgentsProvider);
    final users = ref.watch(userCacheProvider);
    final presence = ref.watch(presenceCacheProvider);
    final deviceNames = ref.watch(agentHostDeviceNamesProvider);
    final currentDevice = ref.watch(currentAccountDeviceIdProvider);
    final robots = ref.watch(deviceRobotOverridesProvider);
    final stopping = useState<Set<String>>(const {});
    final starting = useState<Set<String>>(const {});

    final pubkeys = [for (final agent in agents) agent.pubkey];
    final pubkeysKey = pubkeys.join(',');
    useEffect(() {
      if (pubkeys.isNotEmpty) {
        ref.read(presenceCacheProvider.notifier).track(pubkeys);
        unawaited(ref.read(userCacheProvider.notifier).preload(pubkeys));
      }
      return null;
    }, [pubkeysKey]);

    String nameOf(MyAgent agent) =>
        _nonEmpty(users[agent.pubkey]?.displayName) ??
        agent.directoryName ??
        '${agent.pubkey.substring(0, 8)}…';

    final sorted = [...agents]
      ..sort(
        (a, b) => nameOf(a).toLowerCase().compareTo(nameOf(b).toLowerCase()),
      );

    Future<void> stop(MyAgent agent, String name) async {
      final channelId = agent.commandChannelId;
      if (channelId == null) return;
      final ok = await _confirmStop(context, name);
      if (!ok || !context.mounted) return;
      stopping.value = {...stopping.value, agent.pubkey};
      final messenger = ScaffoldMessenger.of(context);
      try {
        await ref
            .read(sendMessageProvider)
            .call(
              channelId: channelId,
              content: '!shutdown',
              mentionPubkeys: [agent.pubkey],
            );
        messenger.showSnackBar(
          const SnackBar(
            content: Text(
              'Stop requested. The agent stops on the computer it runs on.',
            ),
          ),
        );
      } catch (failure) {
        messenger.showSnackBar(
          SnackBar(content: Text('Couldn’t stop $name: $failure')),
        );
      } finally {
        if (context.mounted) {
          stopping.value = {...stopping.value}..remove(agent.pubkey);
        }
      }
    }

    Future<void> start(MyAgent agent, String name, String computer) async {
      final channelId = agent.commandChannelId;
      if (channelId == null || starting.value.contains(agent.pubkey)) return;
      starting.value = {...starting.value, agent.pubkey};
      final messenger = ScaffoldMessenger.of(context);
      try {
        await ref
            .read(sendMessageProvider)
            .call(
              channelId: channelId,
              content: '!start',
              mentionPubkeys: [agent.pubkey],
            );
        messenger.showSnackBar(
          SnackBar(
            content: Text(
              'Start requested. $name starts only if Kuzz is open on '
              '$computer now.',
            ),
          ),
        );
      } catch (failure) {
        messenger.showSnackBar(
          SnackBar(content: Text('Couldn’t start $name: $failure')),
        );
      } finally {
        if (context.mounted) {
          starting.value = {...starting.value}..remove(agent.pubkey);
        }
      }
    }

    void openSettings(MyAgent agent, String name) => unawaited(
      Navigator.of(context).push(
        MaterialPageRoute<void>(
          builder: (_) =>
              AgentSettingsPage(agentPubkey: agent.pubkey, fallbackName: name),
        ),
      ),
    );

    return Scaffold(
      appBar: AppBar(title: const Text('My agents')),
      body: SafeArea(
        child: ListView(
          padding: const EdgeInsets.symmetric(vertical: Grid.xs),
          children: [
            if (sorted.isEmpty)
              Padding(
                padding: const EdgeInsets.all(Grid.sm),
                child: Text(
                  'Agents you create on your computers show up here.',
                  key: const Key('my-agents-empty'),
                  textAlign: TextAlign.center,
                  style: context.textTheme.bodyMedium?.copyWith(
                    color: context.colors.onSurfaceVariant,
                  ),
                ),
              )
            else
              AppListCard(
                description:
                    'Start and Stop run on the computer each agent lives on. '
                    'That computer’s Kuzz app must be open.',
                children: [
                  for (final agent in sorted)
                    _MyAgentRow(
                      key: ValueKey('my-agent-${agent.pubkey}'),
                      agent: agent,
                      name: nameOf(agent),
                      users: users,
                      presence: presence[agent.pubkey],
                      hostLabel: agentHostDeviceLabel(
                        deviceNames[agent.deviceId],
                        current: currentDevice == agent.deviceId,
                      ),
                      robot: resolveDeviceRobot(
                        agent.deviceId,
                        override: robots[agent.deviceId],
                      ),
                      stopping: stopping.value.contains(agent.pubkey),
                      starting: starting.value.contains(agent.pubkey),
                      onStop: () => unawaited(stop(agent, nameOf(agent))),
                      onStart: () => unawaited(
                        start(
                          agent,
                          nameOf(agent),
                          _computerName(
                            deviceNames[agent.deviceId],
                            current: currentDevice == agent.deviceId,
                          ),
                        ),
                      ),
                      onTap: () => openSettings(agent, nameOf(agent)),
                    ),
                ],
              ),
          ],
        ),
      ),
    );
  }
}

class _MyAgentRow extends StatelessWidget {
  const _MyAgentRow({
    super.key,
    required this.agent,
    required this.name,
    required this.users,
    required this.presence,
    required this.hostLabel,
    required this.robot,
    required this.stopping,
    required this.starting,
    required this.onStop,
    required this.onStart,
    required this.onTap,
  });

  final MyAgent agent;
  final String name;
  final Map<String, UserProfile> users;
  final String? presence;
  final String hostLabel;
  final DeviceRobotVariant? robot;
  final bool stopping;
  final bool starting;
  final VoidCallback onStop;
  final VoidCallback onStart;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final running = presence == 'online' || presence == 'away';
    final status = switch (presence) {
      'online' => 'Online',
      'away' => 'Busy',
      'offline' => 'Stopped',
      _ => 'Checking…',
    };
    final canStop = running && agent.commandChannelId != null && !stopping;
    final canStart = presence == 'offline' && agent.commandChannelId != null;
    return AppListRowRaw(
      onTap: onTap,
      leading: SmallAvatar(pubkey: agent.pubkey, userCache: users, size: 36),
      title: Text(name, style: context.textTheme.bodyLarge),
      subtitle: Row(
        children: [
          Container(
            width: 8,
            height: 8,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              color: switch (presence) {
                'online' => context.appColors.success,
                'away' => context.appColors.warning,
                _ => context.colors.outline,
              },
            ),
          ),
          const SizedBox(width: Grid.half),
          Text(status, style: context.textTheme.bodySmall),
          const SizedBox(width: Grid.xxs),
          DeviceRobotIcon(variant: robot, size: 14),
          const SizedBox(width: Grid.quarter),
          Expanded(
            child: Text(
              hostLabel,
              key: ValueKey('my-agent-host-${agent.pubkey}'),
              overflow: TextOverflow.ellipsis,
              style: context.textTheme.bodySmall?.copyWith(
                color: context.colors.onSurfaceVariant,
              ),
            ),
          ),
        ],
      ),
      trailing: running
          ? TextButton(
              key: ValueKey('my-agent-stop-${agent.pubkey}'),
              onPressed: canStop ? onStop : null,
              child: Text(stopping ? 'Stopping…' : 'Stop'),
            )
          : canStart
          ? TextButton(
              key: ValueKey('my-agent-start-${agent.pubkey}'),
              onPressed: starting ? null : onStart,
              child: Text(starting ? 'Starting…' : 'Start'),
            )
          : null,
    );
  }
}

Future<bool> _confirmStop(BuildContext context, String name) async {
  final confirmed = await showKuzzDialog<bool>(
    context: context,
    builder: (dialogContext) => AlertDialog(
      title: Text('Stop $name?'),
      content: const Text(
        'It stops on the computer it runs on and stays stopped until it is '
        'started there again.',
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(dialogContext).pop(false),
          child: const Text('Cancel'),
        ),
        FilledButton(
          key: const Key('my-agents-confirm-stop'),
          onPressed: () => Navigator.of(dialogContext).pop(true),
          child: const Text('Stop'),
        ),
      ],
    ),
  );
  return confirmed ?? false;
}

String? _nonEmpty(String? value) {
  final trimmed = value?.trim() ?? '';
  return trimmed.isEmpty ? null : trimmed;
}

/// The computer named in the row's host label, for sentences.
String _computerName(String? deviceName, {required bool current}) {
  final name = deviceName?.trim() ?? '';
  if (current) return name.isEmpty ? 'this device' : 'this device ($name)';
  return name.isEmpty ? 'its computer' : name;
}
