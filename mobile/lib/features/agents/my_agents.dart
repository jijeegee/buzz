import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/auth/account/account_api.dart';
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

/// An agent connected from a paired computer (`kuzz pair`, Hermes), not run
/// by a Kuzz app: no start, stop or edit here.
class MyExternalAgent {
  const MyExternalAgent({
    required this.agent,
    required this.externalDeviceId,
    this.runtime,
  });

  /// The agent, with [MyAgent.deviceId] the computer it runs on: the
  /// signed-in device there, else the external device itself.
  final MyAgent agent;

  /// Lowercase id of the external device it connected through.
  final String externalDeviceId;

  /// What it runs on (`Hermes`), when the connection said.
  final String? runtime;
}

/// The agents on the account's external devices ([AccountDevice.agents]),
/// each under its computer as desktop's Agents list does. Sorted by pubkey.
List<MyExternalAgent> myExternalAgentsFrom({
  required List<AccountDevice> devices,
  required List<AgentDirectoryEntry> directory,
}) {
  final byPubkey = {
    for (final entry in directory) entry.pubkey.toLowerCase(): entry,
  };
  final ids = {for (final device in devices) device.id.trim().toLowerCase()};
  final agents = <String, MyExternalAgent>{};
  for (final device in devices) {
    if (!device.isExternal) continue;
    final id = device.id.trim().toLowerCase();
    final login = device.loginDeviceId?.trim().toLowerCase();
    final host = login != null && ids.contains(login) ? login : id;
    for (final agent in device.agents) {
      final pubkey = agent.pubkey.trim().toLowerCase();
      agents.putIfAbsent(
        pubkey,
        () => MyExternalAgent(
          agent: MyAgent(
            pubkey: pubkey,
            deviceId: host,
            directoryName: _nonEmpty(byPubkey[pubkey]?.displayName),
          ),
          externalDeviceId: id,
          runtime: agent.runtime,
        ),
      );
    }
  }
  return agents.values.toList()
    ..sort((a, b) => a.agent.pubkey.compareTo(b.agent.pubkey));
}

/// [myExternalAgentsFrom] for the signed-in user; none until the device
/// list loads, or when it can't be read.
final myExternalAgentsProvider = Provider.autoDispose<List<MyExternalAgent>>((
  ref,
) {
  final sessionKey = ref.watch(accountSessionProvider);
  if (sessionKey == null) return const [];
  return myExternalAgentsFrom(
    devices:
        ref.watch(accountDevicesProvider(sessionKey)).value ??
        const <AccountDevice>[],
    directory:
        ref.watch(agentDirectoryProvider).asData?.value ??
        const <AgentDirectoryEntry>[],
  );
});
