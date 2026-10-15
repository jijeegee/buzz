import { useSyncExternalStore } from "react";

import type { Community } from "./types";
import {
  deriveCommunityName,
  loadActiveCommunityId,
  loadCommunities,
  normalizeRelayUrl,
  saveActiveCommunityId,
  saveCommunities,
} from "./communityStorage";
import { readmitRelay } from "@/features/agents/managedAgentRelayCleanup";
import { invokeTauri } from "@/shared/api/tauri";
import { signingKeyUiTestSeam } from "@/shared/lib/signingKeyUi";

/**
 * The built-in server: the one relay the native side connects to
 * (`get_built_in_relay_url`; the address lives only in Rust). With it the app
 * is a single-server client: no server names, no switching, adding, editing,
 * leaving or removing servers, and invites only for that server.
 *
 * Only E2E mock builds (`vite build --mode e2e`) keep the upstream
 * multi-community controls, so their suites stay valid. Without a native
 * bridge (unit tests, a plain browser) the value is `null` as well.
 */

export const DIFFERENT_SERVICE_INVITE_MESSAGE =
  "This invite is for a different service.";

export type BuiltInServerState =
  | { status: "loading" }
  | { status: "ready"; relayUrl: string | null };

/**
 * Host + port identifying a server, after the app's usual relay URL
 * normalisation. ws/wss/http/https forms of one host:port compare equal;
 * default ports are omitted, as `URL.host` does.
 */
export function serverHostKey(url: string | null | undefined): string | null {
  const trimmed = url?.trim();
  if (!trimmed) return null;
  const withScheme = /^[a-z][a-z0-9+.-]*:\/\//i.test(trimmed)
    ? trimmed
    : `wss://${trimmed}`;
  try {
    const parsed = new URL(withScheme.replace(/^ws(s?):\/\//i, "http$1://"));
    if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
      return null;
    }
    return parsed.host.toLowerCase() || null;
  } catch {
    return null;
  }
}

/** Whether `url` points at the same server (host + port) as `builtIn`. */
export function isSameServer(
  url: string | null | undefined,
  builtIn: string | null | undefined,
): boolean {
  const a = serverHostKey(url);
  return a !== null && a === serverHostKey(builtIn);
}

/**
 * Whether the app may follow an invite, link or account for `relayUrl`. Without
 * a built-in server every host is allowed (upstream behaviour).
 */
export function isAllowedServer(
  relayUrl: string | null | undefined,
  builtIn: string | null,
): boolean {
  return builtIn === null || isSameServer(relayUrl, builtIn);
}

/**
 * What to do with a `buzz://connect|join|add-community` link. With a built-in
 * server, links for another host are rejected (nothing is added or switched)
 * and add-community links for the built-in server are dropped: it is already
 * the only server.
 */
export function communityLinkDecision(
  kind: "connect" | "join" | "add-community",
  relayUrl: string,
  builtIn: string | null,
): "follow" | "ignore" | "reject" {
  if (builtIn === null) return "follow";
  if (!isSameServer(relayUrl, builtIn)) return "reject";
  return kind === "add-community" ? "ignore" : "follow";
}

/**
 * The built-in relay URL reported by the native side, or `null` when the
 * single-server mode is off: no usable value, or an E2E mock build (whose
 * suites drive the upstream controls).
 */
export function resolveBuiltInServer(
  configured: string | null | undefined,
  mode?: string,
): string | null {
  if (signingKeyUiTestSeam(mode)) return null;
  const trimmed = configured?.trim();
  if (!trimmed || serverHostKey(trimmed) === null) return null;
  return trimmed;
}

/**
 * Startup pinning: with a built-in server, the stored community on that
 * server is always the active one. Others stay stored (never deleted) but
 * unused. Without a match (or without a built-in server) the stored choice
 * stands.
 */
export function pinnedActiveCommunityId(
  communities: readonly Community[],
  activeId: string | null,
  builtIn: string | null,
): string | null {
  if (!builtIn) return activeId;
  const current =
    communities.find((community) => community.id === activeId) ??
    communities[0];
  if (current && isSameServer(current.relayUrl, builtIn)) return activeId;
  const match = communities.find((community) =>
    isSameServer(community.relayUrl, builtIn),
  );
  return match ? match.id : activeId;
}

export type StartupPin = {
  communities: Community[];
  activeId: string;
  created: Community | null;
};

/**
 * What startup stores so the built-in server is active: select its stored
 * entry, or add one when the device only holds other servers (they stay
 * stored, never connected). `null` when nothing changes, including a first
 * run with no communities (the default-server onboarding adds it).
 */
export function resolveStartupPin(
  communities: readonly Community[],
  activeId: string | null,
  builtIn: string | null,
  createId: () => string = () => crypto.randomUUID(),
  now: () => Date = () => new Date(),
): StartupPin | null {
  if (!builtIn || communities.length === 0) return null;
  const current =
    communities.find((community) => community.id === activeId) ??
    communities[0];
  if (isSameServer(current.relayUrl, builtIn)) return null;
  const match = communities.find((community) =>
    isSameServer(community.relayUrl, builtIn),
  );
  if (match) {
    return { communities: [...communities], activeId: match.id, created: null };
  }
  const relayUrl = normalizeRelayUrl(builtIn);
  const created: Community = {
    id: createId(),
    name: deriveCommunityName(relayUrl),
    relayUrl,
    pubkey: current.pubkey,
    addedAt: now().toISOString(),
  };
  return {
    communities: [...communities, created],
    activeId: created.id,
    created,
  };
}

/** Persist [`resolveStartupPin`] for the stored communities. */
export function pinStoredCommunitiesToBuiltInServer(
  builtIn: string | null,
): void {
  const pin = resolveStartupPin(
    loadCommunities(),
    loadActiveCommunityId(),
    builtIn,
  );
  if (!pin) return;
  if (pin.created && !saveCommunities(pin.communities)) return;
  saveActiveCommunityId(pin.activeId);
  if (pin.created) {
    // A server removed from this device earlier admits local agents again.
    readmitRelay(pin.created.relayUrl).catch((error) => {
      console.error(
        "[communities] re-admitting local agents on the built-in server failed:",
        error,
      );
    });
  }
}

// ── Shared store ─────────────────────────────────────────────────────────

let state: BuiltInServerState = signingKeyUiTestSeam()
  ? { status: "ready", relayUrl: null }
  : { status: "loading" };
let loading: Promise<string | null> | null = null;
const listeners = new Set<() => void>();

function setState(next: BuiltInServerState) {
  state = next;
  for (const listener of listeners) listener();
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

function getSnapshot(): BuiltInServerState {
  return state;
}

/**
 * Read the built-in server once per app run. A failed native call resolves
 * to `null` (upstream behaviour) rather than blocking startup.
 */
export function loadBuiltInServer(
  readConfigured: () => Promise<string | null> = () =>
    invokeTauri<string>("get_built_in_relay_url"),
  mode?: string,
): Promise<string | null> {
  if (state.status === "ready") return Promise.resolve(state.relayUrl);
  if (!loading) {
    loading = readConfigured()
      .catch(() => null)
      .then((configured) => {
        const relayUrl = resolveBuiltInServer(configured, mode);
        // Stored before anything reads the active community (the app root
        // waits for this), so startup never applies another server.
        try {
          pinStoredCommunitiesToBuiltInServer(relayUrl);
        } catch (error) {
          console.error(
            "[communities] pinning the built-in server failed:",
            error,
          );
        }
        setState({ status: "ready", relayUrl });
        return relayUrl;
      });
  }
  return loading;
}

/** The built-in relay URL, or `null` while loading or when there is none. */
export function getBuiltInServer(): string | null {
  return state.status === "ready" ? state.relayUrl : null;
}

/** Loading state for the startup gate. */
export function useBuiltInServerState(): BuiltInServerState {
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}

/**
 * The built-in relay URL, or `null` when the upstream multi-community
 * controls apply. The app root waits for it before rendering anything that
 * reads it, so `null` here never stands in for "still loading" in practice.
 */
export function useBuiltInServer(): string | null {
  const current = useBuiltInServerState();
  return current.status === "ready" ? current.relayUrl : null;
}

/** Test seam: replace the loaded value (or reset to loading with `undefined`). */
export function setBuiltInServerForTests(relayUrl: string | null | undefined) {
  loading = null;
  setState(
    relayUrl === undefined
      ? { status: "loading" }
      : { status: "ready", relayUrl },
  );
}
