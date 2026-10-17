import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/devices/agent_host_devices.dart';
import '../../shared/mentions/agent_identity_provider.dart';
import '../channels/channel.dart';
import '../channels/channels_provider.dart';

/// One of the signed-in user's agents, as the phone sees it: the device it
/// runs on and a channel where an owner command can reach it.
class MyAgent {
  const MyAgent({
    required this.pubkey,
    required this.deviceId,
    this.directoryName,
    this.commandChannelId,
  });

  /// Lowercase hex pubkey.
  final String pubkey;

  /// Lowercase relay device id of the computer hosting it.
  final String deviceId;

  /// Name from the agent's kind:10100 profile, if it published one.
  final String? directoryName;

  /// Channel to send `!shutdown` in; `null` when the agent is in none that
  /// this user can post to.
  final String? commandChannelId;
}

/// The owner's agents from their kind:30180 host map ([hostDevices]), with
/// a command channel from the agent directory: the first of the agent's
/// channels the owner is a member of, else its first channel (desktop's
/// `resolveManagedAgentChannelId`). Sorted by pubkey; the page sorts by name.
List<MyAgent> myAgentsFrom({
  required Map<String, String> hostDevices,
  required List<AgentDirectoryEntry> directory,
  required List<Channel> channels,
}) {
  final byPubkey = {
    for (final entry in directory) entry.pubkey.toLowerCase(): entry,
  };
  final memberChannels = {
    for (final channel in channels)
      if (channel.isMember && channel.archivedAt == null) channel.id,
  };
  final agents = [
    for (final MapEntry(key: pubkey, value: deviceId) in hostDevices.entries)
      MyAgent(
        pubkey: pubkey.toLowerCase(),
        deviceId: deviceId.toLowerCase(),
        directoryName: _nonEmpty(byPubkey[pubkey.toLowerCase()]?.displayName),
        commandChannelId: _commandChannel(
          byPubkey[pubkey.toLowerCase()]?.channelIds ?? const [],
          memberChannels,
        ),
      ),
  ];
  agents.sort((a, b) => a.pubkey.compareTo(b.pubkey));
  return agents;
}

String? _commandChannel(List<String> agentChannels, Set<String> mine) {
  for (final id in agentChannels) {
    if (mine.contains(id)) return id;
  }
  return agentChannels.isEmpty ? null : agentChannels.first;
}

String? _nonEmpty(String? value) {
  final trimmed = value?.trim() ?? '';
  return trimmed.isEmpty ? null : trimmed;
}

/// [myAgentsFrom] for the signed-in user.
final myAgentsProvider = Provider.autoDispose<List<MyAgent>>((ref) {
  return myAgentsFrom(
    hostDevices: ref.watch(agentHostDevicesProvider),
    directory:
        ref.watch(agentDirectoryProvider).asData?.value ??
        const <AgentDirectoryEntry>[],
    channels: ref.watch(channelsProvider).asData?.value ?? const <Channel>[],
  );
});
