import { useQuery } from "@tanstack/react-query";

import { useIdentityQuery } from "@/shared/api/hooks";

import {
  type AdminProbeResult,
  discoverAdminOrigin,
  probeAdminOrigin,
} from "./api";

type AdminConsoleAccessDeps = {
  discoverOrigin: () => Promise<{ origin: string; sameHost: boolean } | null>;
  probe: (origin: string) => Promise<AdminProbeResult>;
};

const defaultDeps: AdminConsoleAccessDeps = {
  discoverOrigin: discoverAdminOrigin,
  probe: probeAdminOrigin,
};

/**
 * Whether the active identity is an authorized operator or moderator of the
 * connected relay's admin API. Kuzz runs a single relay, so access is checked
 * against the origin that relay advertises for itself; a saved origin is
 * ignored so a stale or mistyped one can never hide the section. A cross-host
 * advertisement is never probed, so this never signs a request for an origin
 * the relay cannot vouch for. Throws when the check could not complete (IPC
 * failure or an unreachable server) so a transient failure is not read as a
 * denial.
 */
export async function resolveAdminConsoleAccess(
  deps: AdminConsoleAccessDeps = defaultDeps,
): Promise<boolean> {
  const discovered = await deps.discoverOrigin();
  if (!discovered?.sameHost) return false;
  const result = await deps.probe(discovered.origin);
  if (result.state === "networkOrIntercepted") {
    throw new Error("Admin access check could not reach the server");
  }
  return result.state === "nip98Authorized";
}

/**
 * Kuzz shows the Admin settings section only to confirmed operators and
 * moderators. The role is checked once per identity per app session; signing
 * out relaunches the app, and switching accounts changes the query key.
 */
export function useAdminConsoleAccessQuery() {
  const { data: identity } = useIdentityQuery();
  const pubkeyHex = identity?.pubkey ?? "";
  return useQuery({
    queryKey: ["admin-console-access", pubkeyHex],
    queryFn: () => resolveAdminConsoleAccess(),
    enabled: pubkeyHex.length > 0,
    staleTime: Number.POSITIVE_INFINITY,
    gcTime: Number.POSITIVE_INFINITY,
    retry: false,
  });
}
