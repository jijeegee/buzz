import { skipToken, useQuery } from "@tanstack/react-query";

import {
  type UsersBatchEntry,
  usersBatchEntryKey,
} from "@/features/profile/hooks";
import { useIdentityQuery } from "@/shared/api/hooks";
import { type AgentBadge, agentBadge } from "@/shared/lib/deviceRobot";

/**
 * What the signed-in viewer sees in an agent's robot slot (see
 * `agentBadge`). Reads the per-pubkey profile entries `useUsersBatchQuery`
 * already resolved for the surrounding list (it never fetches on its own), so
 * a row whose author profile is loaded gets its badge with no extra request.
 */
export function useAgentBadge(
  agentPubkey: string | null | undefined,
): AgentBadge {
  const pubkey = agentPubkey?.toLowerCase() ?? "";
  const entry = useQuery<UsersBatchEntry>({
    queryKey: usersBatchEntryKey(pubkey),
    queryFn: skipToken,
  });
  const identity = useIdentityQuery();
  const summary = pubkey ? entry.data?.summary : null;
  return agentBadge({
    hostDevice: summary?.hostDevice,
    ownerPubkey: summary?.ownerPubkey,
    viewerPubkey: identity.data?.pubkey,
  });
}
