import * as React from "react";
import {
  type QueryClient,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";

import { useUserProfileQuery } from "@/features/profile/hooks";
import {
  type AgentPairing,
  getHermesStatus,
  listAgentPairings,
} from "@/shared/api/agentPairing";
import { useIdentityQuery } from "@/shared/api/hooks";
import { getTokenAuthStatus, onTokenAuthChanged } from "@/shared/api/tokenAuth";
import {
  AUTH_DEVICES_QUERY_KEY,
  useAuthDevicesQuery,
} from "@/shared/api/useAuthDeviceNames";
import { localeDateTimeFormat, t } from "@/shared/i18n";
import {
  EXTERNAL_PLATFORM,
  type ExternalAgentGroup,
  groupExternalAgents,
  mergeRemembered,
  parseRemembered,
  type RememberedPairing,
} from "./pairingLogic";

const AVAILABLE_KEY = ["external-connect-available"] as const;
export const AGENT_PAIRINGS_KEY = ["agent-pairings"] as const;
const HERMES_KEY = ["hermes-status"] as const;
const REMEMBERED_PREFIX = "kuzz.externalAgents.v1.";
const REMEMBERED_EVENT = "kuzz:external-agents-remembered";

/**
 * Whether this account can pair external agents: signed in with Google with
 * its key kept (the relay's pairing calls need that). False otherwise, and
 * on relays without Google sign-in, so nothing new shows there.
 */
export function useExternalConnectAvailable(): boolean {
  const queryClient = useQueryClient();
  const query = useQuery({
    queryKey: AVAILABLE_KEY,
    queryFn: async () => {
      try {
        const status = await getTokenAuthStatus();
        return (
          status.supported &&
          status.state === "active" &&
          status.keyBackup === true
        );
      } catch {
        return false;
      }
    },
    staleTime: 60_000,
  });
  React.useEffect(() => {
    let unlisten: (() => void) | undefined;
    let disposed = false;
    onTokenAuthChanged(() => {
      void queryClient.invalidateQueries({ queryKey: AVAILABLE_KEY });
    })
      .then((fn) => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(() => {
        // No sign-in events here: the status is read once.
      });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [queryClient]);
  return query.data === true;
}

export function useAgentPairingsQuery(enabled: boolean) {
  return useQuery({
    enabled,
    queryKey: AGENT_PAIRINGS_KEY,
    queryFn: listAgentPairings,
    retry: false,
    staleTime: 30_000,
  });
}

export function useHermesStatusQuery(enabled: boolean) {
  return useQuery({
    enabled,
    queryKey: HERMES_KEY,
    queryFn: getHermesStatus,
    retry: false,
    staleTime: 60_000,
  });
}

/** Refresh everything the external agent list shows. */
export function refreshExternalAgents(queryClient: QueryClient) {
  for (const queryKey of [
    AGENT_PAIRINGS_KEY,
    AUTH_DEVICES_QUERY_KEY,
    HERMES_KEY,
  ]) {
    void queryClient.invalidateQueries({ queryKey });
  }
}

function storageKey(owner: string) {
  return `${REMEMBERED_PREFIX}${owner.toLowerCase()}`;
}

function readRemembered(owner: string): RememberedPairing[] {
  if (!owner) return [];
  try {
    return parseRemembered(window.localStorage.getItem(storageKey(owner)));
  } catch {
    return [];
  }
}

function writeRemembered(owner: string, entries: RememberedPairing[]) {
  if (!owner) return;
  try {
    window.localStorage.setItem(storageKey(owner), JSON.stringify(entries));
    window.dispatchEvent(new Event(REMEMBERED_EVENT));
  } catch {
    // Storage full or blocked: the relay's one-day list still shows them.
  }
}

/** Remember a connection seen now (the relay lists claims for one day). */
export function rememberConnection(owner: string, pairing: AgentPairing) {
  const merged = mergeRemembered(readRemembered(owner), [pairing]);
  writeRemembered(owner, merged);
}

function sameEntries(a: RememberedPairing[], b: RememberedPairing[]) {
  return JSON.stringify(a) === JSON.stringify(b);
}

/**
 * The account's paired computers, each with its connected agents, and the
 * Hermes agents this app paired (so they can be named). Empty when external
 * connect is unavailable.
 */
export function useExternalAgentGroups(): {
  groups: ExternalAgentGroup[];
  hermesPubkeys: ReadonlySet<string>;
  owner: string;
  /** The device list loaded (it holds this computer). */
  devicesLoaded: boolean;
} {
  const available = useExternalConnectAvailable();
  const owner = useIdentityQuery().data?.pubkey?.toLowerCase() ?? "";
  const devices = useAuthDevicesQuery(available);
  const pairings = useAgentPairingsQuery(available);
  const hermes = useHermesStatusQuery(available);
  const [remembered, setRemembered] = React.useState<RememberedPairing[]>(() =>
    readRemembered(owner),
  );

  React.useEffect(() => {
    setRemembered(readRemembered(owner));
    const reload = () => setRemembered(readRemembered(owner));
    window.addEventListener(REMEMBERED_EVENT, reload);
    return () => window.removeEventListener(REMEMBERED_EVENT, reload);
  }, [owner]);

  // Keep what the relay listed, and forget computers that were disconnected.
  // The device list counts only when it loaded (it holds this computer).
  const devicesLoaded = (devices.data ?? []).some((device) => device.current);
  React.useEffect(() => {
    if (!available || !owner) return;
    const deviceList = devices.data ?? [];
    const live = devicesLoaded
      ? new Set(
          deviceList
            .filter((device) => device.platform === EXTERNAL_PLATFORM)
            .map((device) => device.id),
        )
      : undefined;
    const current = readRemembered(owner);
    const next = mergeRemembered(current, pairings.data ?? [], live);
    if (!sameEntries(current, next)) writeRemembered(owner, next);
  }, [available, devices.data, devicesLoaded, owner, pairings.data]);

  const groups = React.useMemo(
    () =>
      available
        ? groupExternalAgents(
            devices.data ?? [],
            pairings.data ?? [],
            remembered,
          )
        : [],
    [available, devices.data, pairings.data, remembered],
  );
  const hermesPubkeys = React.useMemo(
    () =>
      new Set(
        (hermes.data?.agents ?? []).map((agent) =>
          agent.agentPubkey.toLowerCase(),
        ),
      ),
    [hermes.data],
  );
  return { groups, hermesPubkeys, owner, devicesLoaded };
}

/** An external agent's name: its profile, "Hermes", or a fallback. */
export function useExternalAgentName(
  pubkey: string | null,
  hermesPubkeys?: ReadonlySet<string>,
): string {
  const profile = useUserProfileQuery(pubkey ?? undefined).data;
  const name = profile?.displayName?.trim();
  if (name) return name;
  if (pubkey && hermesPubkeys?.has(pubkey.toLowerCase())) return "Hermes";
  return t("agents.external.agentFallback");
}

/** `Oct 23, 14:32` in the reader's locale; empty for a missing time. */
export function formatConnectedAt(iso: string | null): string {
  if (!iso) return "";
  const at = new Date(iso);
  if (Number.isNaN(at.getTime())) return "";
  return localeDateTimeFormat({
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  }).format(at);
}
