import 'package:flutter/material.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/l10n/l10n.dart';
import '../../shared/mentions/agent_identity_provider.dart';
import '../../shared/profile/user_cache_provider.dart';
import '../../shared/profile/user_profile.dart';
import '../../shared/widgets/modal_presentation.dart';
import 'channel_management_provider.dart';

/// Of [memberPubkeys], the agents whose verified owner is [me], in order.
///
/// Ownership comes from the agent's verified profile ([profiles]) or the
/// relay agent directory ([agentOwners]); nobody else's agents count.
List<String> ownedAgentPubkeys({
  required Iterable<String> memberPubkeys,
  required String? me,
  required Map<String, String> agentOwners,
  required Map<String, UserProfile> profiles,
}) {
  final owner = me?.trim().toLowerCase();
  if (owner == null || owner.isEmpty) return const [];
  final owned = <String>{};
  for (final raw in memberPubkeys) {
    final pubkey = raw.trim().toLowerCase();
    if (pubkey.isEmpty || pubkey == owner) continue;
    final ownerPubkey = profiles[pubkey]?.ownerPubkey ?? agentOwners[pubkey];
    if (ownerPubkey?.toLowerCase() == owner) owned.add(pubkey);
  }
  return owned.toList();
}

/// My own agents among [channelId]'s current members.
final myAgentsInChannelProvider = Provider.autoDispose
    .family<List<String>, String>((ref, channelId) {
      final members = ref.watch(channelMembersProvider(channelId)).value;
      if (members == null || members.isEmpty) return const [];
      return ownedAgentPubkeys(
        memberPubkeys: members.map((member) => member.pubkey),
        me: ref.watch(currentPubkeyProvider),
        agentOwners: ref.watch(agentOwnersProvider).value ?? const {},
        profiles: ref.watch(userCacheProvider),
      );
    });

/// Whether [me] is the only owner among [members]: the relay refuses that
/// leave, so nothing is removed before trying it.
bool isSoleOwner(Iterable<ChannelMember> members, String? me) {
  final self = me?.toLowerCase();
  if (self == null) return false;
  final owners = members.where((member) => member.isOwner).toList();
  return owners.length == 1 && owners.single.pubkey.toLowerCase() == self;
}

/// My agents to offer removing when I leave [channelId]; empty when I'm its
/// only owner (the leave is refused) or have none there.
List<String> watchAgentsToRemoveOnLeave(WidgetRef ref, String channelId) {
  final members = ref.watch(channelMembersProvider(channelId)).value;
  final agents = ref.watch(myAgentsInChannelProvider(channelId));
  if (members == null || agents.isEmpty) return const [];
  if (isSoleOwner(members, ref.watch(currentPubkeyProvider))) return const [];
  return agents;
}

/// How to leave, picked in [showLeaveConfirmation].
enum LeaveChoice { leaveOnly, removeAgentsAndLeave }

/// Confirms leaving. With [agentCount] of my agents in the room it offers
/// "Leave and remove my agents (N)", "Leave only" and "Cancel"; otherwise a
/// plain Cancel / [confirmLabel] dialog. Null means cancelled.
Future<LeaveChoice?> showLeaveConfirmation({
  required BuildContext context,
  required String title,
  required String body,
  required int agentCount,
  String? confirmLabel,
  Key? confirmKey,
}) => showKuzzDialog<LeaveChoice>(
  context: context,
  builder: (dialogContext) {
    void pop(LeaveChoice? choice) => Navigator.of(dialogContext).pop(choice);
    return AlertDialog(
      title: Text(title),
      content: Text(body),
      actions: agentCount > 0
          ? [
              TextButton(
                key: const ValueKey('leave-cancel'),
                onPressed: () => pop(null),
                child: Text(dialogContext.l10n.commonCancel),
              ),
              TextButton(
                key: const ValueKey('leave-only'),
                onPressed: () => pop(LeaveChoice.leaveOnly),
                child: Text(dialogContext.l10n.channelLeaveOnly),
              ),
              FilledButton(
                key: const ValueKey('leave-remove-agents'),
                onPressed: () => pop(LeaveChoice.removeAgentsAndLeave),
                child: Text(
                  dialogContext.l10n.channelLeaveRemoveAgents(agentCount),
                ),
              ),
            ]
          : [
              TextButton(
                onPressed: () => pop(null),
                child: Text(dialogContext.l10n.commonCancel),
              ),
              FilledButton(
                key: confirmKey,
                onPressed: () => pop(LeaveChoice.leaveOnly),
                child: Text(confirmLabel ?? dialogContext.l10n.channelLeave),
              ),
            ],
    );
  },
);

/// Leaves [channelId] as [choice] says, removing [agents] first when asked.
Future<void> runLeave(
  ChannelActions actions, {
  required String channelId,
  required LeaveChoice choice,
  required List<String> agents,
}) => choice == LeaveChoice.removeAgentsAndLeave && agents.isNotEmpty
    ? actions.removeAgentsAndLeave(channelId: channelId, agentPubkeys: agents)
    : actions.leaveChannel(channelId);
