import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/devices/agent_host_devices.dart';
import '../../shared/mentions/agent_identity_provider.dart';

/// One of the signed-in user's agents, as the phone sees it: the device it
/// runs on.
class MyAgent {
  const MyAgent({
    required this.pubkey,
    required this.deviceId,
    this.directoryName,
  });

  /// Lowercase hex pubkey.
  final String pubkey;

  /// Lowercase relay device id of the computer hosting it.
  final String deviceId;

  /// Name from the agent's kind:10100 profile, if it published one.
  final String? directoryName;
}

/// The owner's agents from their kind:30180 host map ([hostDevices]).
/// Sorted by pubkey; the page sorts by name.
List<MyAgent> myAgentsFrom({
  required Map<String, String> hostDevices,
  required List<AgentDirectoryEntry> directory,
}) {
  final byPubkey = {
    for (final entry in directory) entry.pubkey.toLowerCase(): entry,
  };
  final agents = [
    for (final MapEntry(key: pubkey, value: deviceId) in hostDevices.entries)
      MyAgent(
        pubkey: pubkey.toLowerCase(),
        deviceId: deviceId.toLowerCase(),
        directoryName: _nonEmpty(byPubkey[pubkey.toLowerCase()]?.displayName),
      ),
  ];
  agents.sort((a, b) => a.pubkey.compareTo(b.pubkey));
  return agents;
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
  );
});
