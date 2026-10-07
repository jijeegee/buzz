import { skipToken, useQuery } from "@tanstack/react-query";

import {
  type UsersBatchEntry,
  usersBatchEntryKey,
} from "@/features/profile/hooks";
import { useIdentityQuery } from "@/shared/api/hooks";
import {
  agentDeviceRobotVariant,
  type DeviceRobotVariant,
} from "@/shared/lib/deviceRobot";

/**
 * The device robot an agent shows to the signed-in viewer, or `null` for the
 * default robot. Reads the per-pubkey profile entries `useUsersBatchQuery`
 * already resolved for the surrounding list (it never fetches on its own), so
 * a row whose author profile is loaded gets its robot with no extra request.
 */
export function useAgentDeviceRobot(
  agentPubkey: string | null | undefined,
): DeviceRobotVariant | null {
  const pubkey = agentPubkey?.toLowerCase() ?? "";
  const entry = useQuery<UsersBatchEntry>({
    queryKey: usersBatchEntryKey(pubkey),
    queryFn: skipToken,
  });
  const identity = useIdentityQuery();
  const summary = pubkey ? entry.data?.summary : null;
  return agentDeviceRobotVariant({
    hostDevice: summary?.hostDevice,
    ownerPubkey: summary?.ownerPubkey,
    viewerPubkey: identity.data?.pubkey,
  });
}
