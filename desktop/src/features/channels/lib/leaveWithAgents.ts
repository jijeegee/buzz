import type { ChannelMember } from "@/shared/api/types";
import { normalizePubkey } from "@/shared/lib/pubkey";

/**
 * True when `currentPubkey` is the room's only owner. The relay refuses that
 * leave, so the leave flow must not remove agents first.
 */
export function isSoleOwner(
  members: readonly ChannelMember[],
  currentPubkey: string | null | undefined,
): boolean {
  if (!currentPubkey) return false;
  const self = normalizePubkey(currentPubkey);
  const owners = members.filter((member) => member.role === "owner");
  return owners.length === 1 && normalizePubkey(owners[0].pubkey) === self;
}

/**
 * Agents I own (my managed agents) that are members of the room I am leaving.
 * Empty when I am the sole owner: that leave is refused, so nothing is removed.
 */
export function ownAgentsToRemoveOnLeave(
  members: readonly ChannelMember[],
  managedAgentPubkeys: ReadonlySet<string>,
  currentPubkey: string | null | undefined,
): string[] {
  if (!currentPubkey || isSoleOwner(members, currentPubkey)) return [];
  const self = normalizePubkey(currentPubkey);
  const agents = new Set<string>();
  for (const member of members) {
    const pubkey = normalizePubkey(member.pubkey);
    if (pubkey !== self && managedAgentPubkeys.has(pubkey)) agents.add(pubkey);
  }
  return [...agents];
}

/**
 * Remove each of my agents from the room (kind:9001, one at a time), then
 * leave (kind:9022). A failed removal — typically an agent that is already
 * gone — does not stop the rest or the leave; its pubkey is returned so the
 * caller can say which agents stayed.
 */
export async function leaveRemovingAgents({
  agentPubkeys,
  leave,
  removeMember,
}: {
  agentPubkeys: readonly string[];
  leave: () => Promise<void>;
  removeMember: (pubkey: string) => Promise<void>;
}): Promise<{ failedAgentRemovals: string[] }> {
  const failedAgentRemovals: string[] = [];
  for (const pubkey of agentPubkeys) {
    try {
      await removeMember(pubkey);
    } catch {
      failedAgentRemovals.push(pubkey);
    }
  }
  await leave();
  return { failedAgentRemovals };
}
