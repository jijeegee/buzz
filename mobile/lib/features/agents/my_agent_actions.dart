import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/devices/agent_host_devices.dart';
import '../../shared/l10n/l10n.dart';
import '../../shared/theme/theme.dart';
import '../../shared/widgets/modal_presentation.dart';
import '../channels/channel.dart';
import '../channels/channel_detail_page.dart';
import '../channels/channel_management_provider.dart';
import '../channels/channels_provider.dart';
import '../chats/chat_list_provider.dart';
import '../chats/chat_row_channel.dart';
import '../../shared/relay/relay.dart';
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
  MyAgentStatus.online => appL10n.agentsStatusOnline,
  MyAgentStatus.busy => appL10n.agentsStatusBusy,
  MyAgentStatus.stopped => appL10n.agentsStatusStopped,
  MyAgentStatus.checking => appL10n.agentsStatusChecking,
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
  if (current) {
    return name.isEmpty
        ? appL10n.agentsComputerThisDevice
        : appL10n.agentsComputerThisDeviceNamed(name);
  }
  return name.isEmpty ? appL10n.agentsComputerUnknown : name;
}

/// The computer an agent runs on, as a title: its name, or "This device"
/// when it is the one in hand.
String myAgentComputerLabel(String? deviceName, {required bool current}) {
  final name = deviceName?.trim() ?? '';
  if (current) return appL10n.agentsComputerThisDeviceTitle;
  return name.isEmpty ? appL10n.agentsComputerAnother : name;
}

/// Whether Stop can be sent: the agent runs.
bool myAgentCanStop(MyAgent agent, MyAgentStatus status) =>
    status == MyAgentStatus.online || status == MyAgentStatus.busy;

/// Whether Start can be sent: the agent is known to be stopped.
bool myAgentCanStart(MyAgent agent, MyAgentStatus status) =>
    status == MyAgentStatus.stopped;

/// The channel id of the owner's DM with an agent, opened if there is none.
typedef MyAgentCommandChannel = Future<String> Function(String agentPubkey);

/// Owner commands go to the DM with the agent, so no one else sees them and
/// an agent in no channel can still be reached.
final myAgentCommandChannelProvider = Provider<MyAgentCommandChannel>(
  (ref) => (agentPubkey) async {
    final dm = await ref
        .read(channelActionsProvider)
        .openDm(pubkeys: [agentPubkey]);
    return dm.id;
  },
);

/// The owner's DM with an agent, opened if there is none.
typedef MyAgentDm = Future<Channel> Function(String agentPubkey);

final myAgentDmProvider = Provider<MyAgentDm>(
  (ref) =>
      (agentPubkey) =>
          ref.read(channelActionsProvider).openDm(pubkeys: [agentPubkey]),
);

/// Opens the owner's DM with an agent: what tapping the agent does.
Future<void> openMyAgentDm(
  BuildContext context,
  WidgetRef ref,
  String agentPubkey,
  String name,
) async {
  final navigator = Navigator.of(context);
  final messenger = ScaffoldMessenger.of(context);
  final l10n = context.l10n;
  // A DM the device already knows (saved or loaded list, or a Chats row)
  // opens at once; only a new one waits for the relay.
  final known = _knownDmWith(ref, agentPubkey);
  final Channel channel;
  try {
    channel = known ?? await ref.read(myAgentDmProvider)(agentPubkey);
  } catch (failure) {
    messenger.showSnackBar(
      SnackBar(content: Text(l10n.agentsOpenDmError(name, '$failure'))),
    );
    return;
  }
  await navigator.push(
    MaterialPageRoute<void>(
      builder: (_) => ChannelDetailPage(channel: channel),
    ),
  );
}

/// The 1:1 DM between this account and [agentPubkey] that the channel list
/// or a Chats row already holds, if any.
Channel? _knownDmWith(WidgetRef ref, String agentPubkey) {
  final me = ref.read(myPubkeyProvider)?.toLowerCase();
  if (me == null) return null;
  final pair = {me, agentPubkey.toLowerCase()};
  bool isPair(Iterable<String> people) =>
      setEquals({for (final pubkey in people) pubkey.toLowerCase()}, pair);
  for (final channel in ref.read(channelsProvider).value ?? const <Channel>[]) {
    if (channel.isDm && isPair(channel.participantPubkeys)) return channel;
  }
  for (final row in ref.read(chatListProvider).value?.rows ?? const []) {
    if (!row.isThread && row.isDm && isPair(row.dmMembers)) {
      return channelFromChatRow(row);
    }
  }
  return null;
}

/// Asks the agent to stop after confirming. Stop sends the owner's
/// `!shutdown` in their DM with the agent, like desktop's remote stop; the
/// agent's harness on its computer acts on it.
Future<void> stopMyAgent(
  BuildContext context,
  WidgetRef ref,
  MyAgent agent,
  String name,
) async {
  final commands = ref.read(myAgentCommandsProvider.notifier);
  if (ref.read(myAgentCommandsProvider).containsKey(agent.pubkey)) return;
  final ok = await _confirmStop(context, name);
  if (!ok || !context.mounted) return;
  commands._set(agent.pubkey, MyAgentCommand.stop);
  final messenger = ScaffoldMessenger.of(context);
  final l10n = context.l10n;
  try {
    final channelId = await ref.read(myAgentCommandChannelProvider)(
      agent.pubkey,
    );
    await ref
        .read(sendMessageProvider)
        .call(
          channelId: channelId,
          content: '!shutdown',
          mentionPubkeys: [agent.pubkey],
        );
    messenger.showSnackBar(SnackBar(content: Text(l10n.agentsStopRequested)));
  } catch (failure) {
    messenger.showSnackBar(
      SnackBar(content: Text(l10n.agentsStopError(name, '$failure'))),
    );
  } finally {
    commands._set(agent.pubkey, null);
  }
}

/// Asks the agent's computer to start it with the owner's `!start` in
/// their DM with the agent.
Future<void> startMyAgent(
  BuildContext context,
  WidgetRef ref,
  MyAgent agent,
  String name,
) async {
  final commands = ref.read(myAgentCommandsProvider.notifier);
  if (ref.read(myAgentCommandsProvider).containsKey(agent.pubkey)) return;
  final computer = myAgentComputerName(
    ref.read(agentHostDeviceNamesProvider)[agent.deviceId],
    current: ref.read(currentAccountDeviceIdProvider) == agent.deviceId,
  );
  commands._set(agent.pubkey, MyAgentCommand.start);
  final messenger = ScaffoldMessenger.of(context);
  final l10n = context.l10n;
  try {
    final channelId = await ref.read(myAgentCommandChannelProvider)(
      agent.pubkey,
    );
    await ref
        .read(sendMessageProvider)
        .call(
          channelId: channelId,
          content: '!start',
          mentionPubkeys: [agent.pubkey],
        );
    messenger.showSnackBar(
      SnackBar(content: Text(l10n.agentsStartRequested(name, computer))),
    );
  } catch (failure) {
    messenger.showSnackBar(
      SnackBar(content: Text(l10n.agentsStartError(name, '$failure'))),
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
      title: Text(dialogContext.l10n.agentsStopConfirmTitle(name)),
      content: Text(dialogContext.l10n.agentsStopConfirmBody),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(dialogContext).pop(false),
          child: Text(dialogContext.l10n.agentsCancel),
        ),
        FilledButton(
          key: const Key('my-agents-confirm-stop'),
          onPressed: () => Navigator.of(dialogContext).pop(true),
          child: Text(dialogContext.l10n.agentsStop),
        ),
      ],
    ),
  );
  return confirmed ?? false;
}
