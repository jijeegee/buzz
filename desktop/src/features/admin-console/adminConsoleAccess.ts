import { useQuery } from "@tanstack/react-query";

import { useIdentityQuery } from "@/shared/api/hooks";

import {
  type AdminProbeResult,
  discoverAdminOrigin,
  getAdminOrigin,
  probeAdminOrigin,
} from "./api";

type AdminConsoleAccessDeps = {
  getSavedOrigin: (pubkeyHex: string) => Promise<string | null>;
  discoverOrigin: () => Promise<{ origin: string; sameHost: boolean } | null>;
  probe: (origin: string) => Promise<AdminProbeResult>;
};

const defaultDeps: AdminConsoleAccessDeps = {
  getSavedOrigin: getAdminOrigin,
  discoverOrigin: discoverAdminOrigin,
  probe: probeAdminOrigin,
};

/**
 * Whether the active identity is an authorized operator or moderator of the
 * connected relay's admin API. Mirrors the origin choice the Admin settings
 * card makes on mount: a saved origin, else a same-host relay advertisement.
 * A cross-host advertisement is never probed, so this never signs a request
 * for an origin the relay cannot vouch for. Any failure means "no access".
 */
export async function resolveAdminConsoleAccess(
  pubkeyHex: string,
  deps: AdminConsoleAccessDeps = defaultDeps,
): Promise<boolean> {
  try {
    let origin = await deps.getSavedOrigin(pubkeyHex);
    if (!origin) {
      const discovered = await deps.discoverOrigin();
      origin = discovered?.sameHost ? discovered.origin : null;
    }
    if (!origin) return false;
    const result = await deps.probe(origin);
    return result.state === "nip98Authorized";
  } catch {
    return false;
  }
}

/**
 * Kuzz shows the Admin settings section only to confirmed operators and
 * moderators; while this is loading or on any error it reads as `false`.
 */
export function useAdminConsoleAccessQuery() {
  const { data: identity } = useIdentityQuery();
  const pubkeyHex = identity?.pubkey ?? "";
  return useQuery({
    queryKey: ["admin-console-access", pubkeyHex],
    queryFn: () => resolveAdminConsoleAccess(pubkeyHex),
    enabled: pubkeyHex.length > 0,
    staleTime: 5 * 60_000,
    retry: false,
  });
}
