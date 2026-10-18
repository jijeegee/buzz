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
/// a command channel: the first of the owner's channels whose relay member
/// list ([channelMembers], channel id → member pubkeys) includes the agent,
/// streams before DMs, like desktop's membership-based
/// `resolveManagedAgentChannelId`. Agents' kind:10100 profiles no longer
/// list channels on our relay, so the directory's `channel_ids` (the first
/// the owner is a member of, else the first) is only a fallback. Sorted by
/// pubkey; the page sorts by name.
List<MyAgent> myAgentsFrom({
  required Map<String, String> hostDevices,
  required List<AgentDirectoryEntry> directory,
  required List<Channel> channels,
  Map<String, Iterable<String>> channelMembers = const {},
}) {
  final byPubkey = {
    for (final entry in directory) entry.pubkey.toLowerCase(): entry,
  };
  final memberChannels = {
    for (final channel in channels)
      if (channel.isMember && channel.archivedAt == null) channel.id,
  };
  final ownChannels = [
    for (final channel in channels)
      if (channel.isMember && channel.archivedAt == null && !channel.isDm)
        channel.id,
    for (final channel in channels)
      if (channel.isMember && channel.archivedAt == null && channel.isDm)
        channel.id,
  ];
  final agentChannels = <String, String>{};
  for (final channelId in ownChannels) {
    for (final member in channelMembers[channelId] ?? const <String>[]) {
      agentChannels.putIfAbsent(member.toLowerCase(), () => channelId);
    }
  }
  final agents = [
    for (final MapEntry(key: pubkey, value: deviceId) in hostDevices.entries)
      MyAgent(
        pubkey: pubkey.toLowerCase(),
        deviceId: deviceId.toLowerCase(),
        directoryName: _nonEmpty(byPubkey[pubkey.toLowerCase()]?.displayName),
        commandChannelId:
            agentChannels[pubkey.toLowerCase()] ??
            _commandChannel(
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
  final channels =
      ref.watch(channelsProvider).asData?.value ?? const <Channel>[];
  // Snapshots are cached before the channel list is published, so they are
  // current whenever [channels] is.
  final notifier = ref.read(channelsProvider.notifier);
  return myAgentsFrom(
    hostDevices: ref.watch(agentHostDevicesProvider),
    directory:
        ref.watch(agentDirectoryProvider).asData?.value ??
        const <AgentDirectoryEntry>[],
    channels: channels,
    channelMembers: {
      for (final channel in channels)
        if (channel.isMember)
          channel.id: [
            for (final member in notifier.cachedMembersForChannel(channel.id))
              member.pubkey,
          ],
    },
  );
});
