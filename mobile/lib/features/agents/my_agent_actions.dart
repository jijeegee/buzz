import 'dart:async';

import 'package:flutter/material.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/devices/agent_host_devices.dart';
import '../../shared/theme/theme.dart';
import '../../shared/widgets/modal_presentation.dart';
import '../channels/send_message_provider.dart';
import 'agent_settings_page.dart';
import 'my_agents.dart';

/// What the phone knows about one of the owner's agents right now.
enum MyAgentStatus { online, busy, stopped, checking }

/// [MyAgentStatus] from a presence value (`online`, `away`, `offline`).
MyAgentStatus myAgentStatus(String? presence) => switch (presence) {
  'online' => MyAgentStatus.online,
  'away' => MyAgentStatus.busy,
  'offline' => MyAgentStatus.stopped,
  _ => MyAgentStatus.checking,
};

String myAgentStatusLabel(MyAgentStatus status) => switch (status) {
  MyAgentStatus.online => 'Online',
  MyAgentStatus.busy => 'Busy',
  MyAgentStatus.stopped => 'Stopped',
  MyAgentStatus.checking => 'Checking…',
};

Color myAgentStatusColor(BuildContext context, MyAgentStatus status) =>
    switch (status) {
      MyAgentStatus.online => context.appColors.success,
      MyAgentStatus.busy => context.appColors.warning,
      _ => context.colors.outline,
    };

/// A Start or Stop the phone has sent and not yet finished sending.
enum MyAgentCommand { start, stop }

/// Commands in flight by agent pubkey, shared by every screen that offers
/// Start and Stop so a button reads "Starting…" wherever it shows.
class MyAgentCommandsNotifier extends Notifier<Map<String, MyAgentCommand>> {
  @override
  Map<String, MyAgentCommand> build() => const {};

  void _set(String pubkey, MyAgentCommand? command) {
    final next = {...state};
    if (command == null) {
      next.remove(pubkey);
    } else {
      next[pubkey] = command;
    }
    state = next;
  }
}

final myAgentCommandsProvider =
    NotifierProvider<MyAgentCommandsNotifier, Map<String, MyAgentCommand>>(
      MyAgentCommandsNotifier.new,
    );

/// The computer an agent runs on, for sentences.
String myAgentComputerName(String? deviceName, {required bool current}) {
  final name = deviceName?.trim() ?? '';
  if (current) return name.isEmpty ? 'this device' : 'this device ($name)';
  return name.isEmpty ? 'its computer' : name;
}

/// The computer an agent runs on, as a title: its name, or "This device"
/// when it is the one in hand.
String myAgentComputerLabel(String? deviceName, {required bool current}) {
  final name = deviceName?.trim() ?? '';
  if (current) return 'This device';
  return name.isEmpty ? 'Another computer' : name;
}

/// Whether Stop can be sent: the agent runs and has a channel for the
/// owner's command.
bool myAgentCanStop(MyAgent agent, MyAgentStatus status) =>
    (status == MyAgentStatus.online || status == MyAgentStatus.busy) &&
    agent.commandChannelId != null;

/// Whether Start can be sent: the agent is known to be stopped and has a
/// channel for the owner's command.
bool myAgentCanStart(MyAgent agent, MyAgentStatus status) =>
    status == MyAgentStatus.stopped && agent.commandChannelId != null;

/// Asks the agent to stop after confirming. Stop sends the owner's
/// `!shutdown` in a channel the agent is in, like desktop's remote stop; the
/// Kuzz app on the agent's computer acts on it.
Future<void> stopMyAgent(
  BuildContext context,
  WidgetRef ref,
  MyAgent agent,
  String name,
) async {
  final channelId = agent.commandChannelId;
  final commands = ref.read(myAgentCommandsProvider.notifier);
  if (channelId == null ||
      ref.read(myAgentCommandsProvider).containsKey(agent.pubkey)) {
    return;
  }
  final ok = await _confirmStop(context, name);
  if (!ok || !context.mounted) return;
  commands._set(agent.pubkey, MyAgentCommand.stop);
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
    commands._set(agent.pubkey, null);
  }
}

/// Asks the agent's computer to start it with the owner's `!start`.
Future<void> startMyAgent(
  BuildContext context,
  WidgetRef ref,
  MyAgent agent,
  String name,
) async {
  final channelId = agent.commandChannelId;
  final commands = ref.read(myAgentCommandsProvider.notifier);
  if (channelId == null ||
      ref.read(myAgentCommandsProvider).containsKey(agent.pubkey)) {
    return;
  }
  final computer = myAgentComputerName(
    ref.read(agentHostDeviceNamesProvider)[agent.deviceId],
    current: ref.read(currentAccountDeviceIdProvider) == agent.deviceId,
  );
  commands._set(agent.pubkey, MyAgentCommand.start);
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
    commands._set(agent.pubkey, null);
  }
}

/// Opens the agent's settings (name, description, instructions, model).
void openMyAgentSettings(BuildContext context, String pubkey, String name) =>
    unawaited(
      Navigator.of(context).push(
        MaterialPageRoute<void>(
          builder: (_) =>
              AgentSettingsPage(agentPubkey: pubkey, fallbackName: name),
        ),
      ),
    );

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
