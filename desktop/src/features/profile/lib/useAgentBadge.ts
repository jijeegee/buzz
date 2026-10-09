import { skipToken, useQuery } from "@tanstack/react-query";

import {
  type UsersBatchEntry,
  usersBatchEntryKey,
} from "@/features/profile/hooks";
import { useIdentityQuery } from "@/shared/api/hooks";
import { useOwnerDevices } from "@/shared/api/useOwnerDevices";
import { type AgentBadge, agentBadge } from "@/shared/lib/deviceRobot";
import { normalizePubkey } from "@/shared/lib/pubkey";

/**
 * What the signed-in viewer sees in an agent's robot slot (see
 * `agentBadge`). Reads the per-pubkey profile entries `useUsersBatchQuery`
 * already resolved for the surrounding list (it never fetches a profile on
 * its own), so a row whose author profile is loaded gets its badge with no
 * extra request. When the viewer owns the agent, the device it runs on comes
 * from the viewer's own device records (`useOwnerDevices`), drawn with the
 * robot they picked for that device, if any.
 *
 * `ownerPubkeyHint` is the owner a surrounding row already knows (search
 * results, mention candidates), used when the profile entry is not cached so
 * those rows show the same badge a chat message would.
 */
export function useAgentBadge(
  agentPubkey: string | null | undefined,
  ownerPubkeyHint?: string | null,
): AgentBadge {
  const pubkey = normalizePubkey(agentPubkey ?? "");
  const entry = useQuery<UsersBatchEntry>({
    queryKey: usersBatchEntryKey(pubkey),
    queryFn: skipToken,
  });
  const identity = useIdentityQuery();
  const viewer = normalizePubkey(identity.data?.pubkey ?? "");
  const profileOwner = pubkey ? entry.data?.summary?.ownerPubkey : null;
  const owner = normalizePubkey(profileOwner ?? ownerPubkeyHint ?? "");
  const viewerOwnsAgent =
    pubkey.length > 0 && viewer.length > 0 && owner === viewer;
  const ownerDevices = useOwnerDevices(viewerOwnsAgent);
  if (viewerOwnsAgent) {
    const hostDeviceId = ownerDevices.hostDevices.get(pubkey);
    return agentBadge({
      hostDeviceId,
      robotOverride: hostDeviceId
        ? ownerDevices.robotOverrides.get(hostDeviceId)
        : null,
      ownerPubkey: owner,
      viewerPubkey: viewer,
    });
  }
  return agentBadge({
    hostDeviceId: null,
    ownerPubkey: owner || null,
    viewerPubkey: viewer,
  });
}
