import { relayClient } from "@/shared/api/relayClient";
import type {
  ConnectionState,
  RelaySubscriptionFilter,
} from "@/shared/api/relayClientShared";
import {
  CommunityLiveBundle,
  type CommunityListenerOptions,
  communityScopeOf,
  DESKTOP_COMMUNITY_ROWS,
} from "@/shared/api/relayCommunityBundle";
import type { RelayEvent } from "@/shared/api/types";

const bundle = new CommunityLiveBundle({
  rows: DESKTOP_COMMUNITY_ROWS,
  open: (filters, onEvent, onStatus, signal, onGone) =>
    relayClient.subscribeLiveFilters(
      filters,
      onEvent,
      () => onStatus("ready"),
      signal,
      onGone,
    ),
  setTimeout: (fn, ms) => globalThis.setTimeout(fn, ms) as unknown as number,
  clearTimeout: (id) => globalThis.clearTimeout(id),
  nowMs: () => Date.now(),
  warn: (message) => console.warn(message),
});

let stopWatchingConnection: (() => void) | null = null;

/** `disconnect()` (sign-out, community switch) resets the bundle. */
function watchConnection() {
  if (stopWatchingConnection) return;
  let previous: ConnectionState | null = null;
  stopWatchingConnection = relayClient.subscribeToConnectionState((state) => {
    // The first call replays the current state; only a change counts.
    if (previous !== null && previous !== "idle" && state === "idle") {
      bundle.reset();
    }
    previous = state;
  });
}

/**
 * The community bundle (`CommunityLiveBundle`): community-wide live filters
 * on one REQ. A filter `communityScopeOf` accepts rides it; any other gets
 * its own REQ as before, and `options` never fire for it. An object so tests
 * can stub it.
 */
export const communityLive = {
  subscribe(
    filter: RelaySubscriptionFilter,
    onEvent: (event: RelayEvent) => void,
    options?: CommunityListenerOptions,
  ): Promise<() => Promise<void>> {
    if (!communityScopeOf(filter)) {
      return relayClient.subscribeLive(filter, onEvent);
    }
    watchConnection();
    return bundle.subscribe(filter, onEvent, options);
  },
  /** See `CommunityLiveBundle.addPresenceAuthors`. */
  addPresenceAuthors(pubkeys: Iterable<string>): string[] {
    return bundle.addPresenceAuthors(pubkeys);
  },
};
