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
 * multi-community controls (value `null`), so their suites stay valid. When
 * the native call fails, the app shows a connection error with Retry; it
 * never falls back to the multi-community controls.
 */

export const DIFFERENT_SERVICE_INVITE_MESSAGE =
  "This invite is for a different service.";

export type BuiltInServerState =
  | { status: "loading" }
  | { status: "error" }
  | { status: "ready"; relayUrl: string | null };

/**
 * Scheme kind + host + port identifying a server, after the app's usual relay
 * URL normalisation: its `http(s)` origin. `wss` and `https` (or `ws` and
 * `http`) forms of one host:port compare equal, but an unencrypted address
 * never matches an encrypted one. A bare host means `wss`.
 */
export function serverKey(url: string | null | undefined): string | null {
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
    return parsed.host ? parsed.origin.toLowerCase() : null;
  } catch {
    return null;
  }
}

/**
 * Whether `url` points at the same server as `builtIn`: host + port over the
 * same kind of scheme. The built-in server is `wss`, so `ws`/`http` never
 * match it; only a dev build's unencrypted override is reached over `ws`.
 */
export function isSameServer(
  url: string | null | undefined,
  builtIn: string | null | undefined,
): boolean {
  const a = serverKey(url);
  return a !== null && a === serverKey(builtIn);
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
 * Refuse, with {@link DIFFERENT_SERVICE_INVITE_MESSAGE}, any request the
 * webview would send to `relayUrl` unless it is the built-in server. Fails
 * closed: before the built-in server is known, or when reading it failed,
 * nothing is allowed. Only E2E mock builds (no built-in server) allow every
 * host.
 */
export function ensureAllowedServer(relayUrl: string): void {
  const allowed =
    state.status === "ready" && isAllowedServer(relayUrl, state.relayUrl);
  if (!allowed) throw new Error(DIFFERENT_SERVICE_INVITE_MESSAGE);
}

/**
 * What to do with a `kuzz://connect|join|add-community` link. With a built-in
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
 * The deep-link screen (`shouldFollowCommunityLink`): whether to follow a
 * community link, calling `onReject` for a link to another server.
 */
export function screenCommunityLink(
  link: { kind: "connect" | "join" | "add-community"; relayUrl: string },
  builtIn: string | null,
  onReject: () => void,
): boolean {
  const decision = communityLinkDecision(link.kind, link.relayUrl, builtIn);
  if (decision === "reject") onReject();
  return decision === "follow";
}

/**
 * The built-in relay URL reported by the native side, or `null`: an E2E mock
 * build (whose suites drive the upstream controls), or no usable value (which
 * `loadBuiltInServer` treats as a failure, never as the upstream mode).
 */
export function resolveBuiltInServer(
  configured: string | null | undefined,
  mode?: string,
): string | null {
  if (signingKeyUiTestSeam(mode)) return null;
  const trimmed = configured?.trim();
  if (!trimmed || serverKey(trimmed) === null) return null;
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
  // A fresh entry: nothing is copied from the other server's entry (its
  // account is not this server's), so it starts signed out.
  const relayUrl = normalizeRelayUrl(builtIn);
  const created: Community = {
    id: createId(),
    name: deriveCommunityName(relayUrl),
    relayUrl,
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

const readBuiltInRelayUrl = () => invokeTauri<string>("get_built_in_relay_url");

/**
 * Read the built-in server once per app run. A failed native call (or an
 * unusable value) leaves the state `error` and resolves to `null`: the app
 * root shows a connection error with Retry (`retryBuiltInServer`), never the
 * upstream multi-community controls.
 */
export function loadBuiltInServer(
  readConfigured: () => Promise<string | null> = readBuiltInRelayUrl,
  mode?: string,
): Promise<string | null> {
  if (state.status === "ready") return Promise.resolve(state.relayUrl);
  if (!loading) {
    const read: Promise<string | null> = signingKeyUiTestSeam(mode)
      ? Promise.resolve(null)
      : readConfigured().then((configured) => {
          const relayUrl = resolveBuiltInServer(configured, mode);
          if (relayUrl === null) {
            throw new Error("no usable built-in server address");
          }
          return relayUrl;
        });
    loading = read.then(
      (relayUrl) => {
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
      },
      (error) => {
        console.error(
          "[communities] reading the built-in server failed:",
          error,
        );
        loading = null;
        setState({ status: "error" });
        return null;
      },
    );
  }
  return loading;
}

/** Read the built-in server again after a failure (the error screen's Retry). */
export function retryBuiltInServer(
  readConfigured: () => Promise<string | null> = readBuiltInRelayUrl,
  mode?: string,
): Promise<string | null> {
  if (state.status === "error") setState({ status: "loading" });
  return loadBuiltInServer(readConfigured, mode);
}

/**
 * The built-in relay URL, or `null` while loading, after a failed read, or
 * when there is none (E2E mock builds).
 */
export function getBuiltInServer(): string | null {
  return state.status === "ready" ? state.relayUrl : null;
}

/** Loading state for the startup gate. */
export function useBuiltInServerState(): BuiltInServerState {
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}

/**
 * The built-in relay URL, or `null` when the upstream multi-community
 * controls apply. The app root renders nothing that reads it until it is
 * ready (a failed read shows the connection error instead), so `null` here
 * never stands in for "still loading" or "failed" in practice.
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
