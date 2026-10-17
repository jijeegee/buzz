import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/devices/agent_host_devices.dart';
import '../../shared/mentions/agent_identity_provider.dart';
import '../../shared/relay/relay.dart';
import '../pulse/pulse_provider.dart';

/// Every pubkey known to be an agent, lowercase: the relay's agent list, the
/// agent directory, and my own agents (the host map my computers publish,
/// including agents of computers that are signed out). People, Friends and
/// people search leave these out; agents have their own places.
final peopleAgentPubkeysProvider = Provider<Set<String>>((ref) {
  final me = ref.watch(myPubkeyProvider);
  final mine = me == null || me.isEmpty
      ? const <String>{}
      : agentHostDevicesFromEvents(
          ref.watch(ownDeviceEventsProvider).values,
          owner: me,
        ).keys;
  final directory =
      ref.watch(agentDirectoryProvider).value ?? const <AgentDirectoryEntry>[];
  final relayAgents = ref.watch(agentPubkeysProvider).value ?? const <String>[];
  return {
    for (final pk in relayAgents) pk.toLowerCase(),
    for (final entry in directory) entry.pubkey.toLowerCase(),
    for (final pk in mine) pk.toLowerCase(),
  };
});
