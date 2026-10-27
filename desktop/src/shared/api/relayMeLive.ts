import { relayClient } from "@/shared/api/relayClient";
import type {
  ConnectionState,
  LiveSubscriptionReadiness,
  RelaySubscriptionFilter,
} from "@/shared/api/relayClientShared";
import { MeLiveBundle, meScopeOf } from "@/shared/api/relayMeBundle";
import { RECONNECT_REPLAY_CHANNEL_LOOKBACK_SECS } from "@/shared/api/relayReconnectReplay";
import type { RelayEvent } from "@/shared/api/types";

const bundle = new MeLiveBundle({
  open: (filter, onEvent, onReady, signal) =>
    relayClient.subscribeLive(filter, onEvent, onReady, undefined, signal),
  fetch: (filter) => relayClient.fetchEvents(filter),
  setTimeout: (fn, ms) => globalThis.setTimeout(fn, ms) as unknown as number,
});

let stopWatchingConnection: (() => void) | null = null;

/**
 * Catch the bundle up on what it missed while the connection was down, and
 * drop it with the session: `disconnect()` (sign-out, community switch) goes
 * idle, drops every live REQ and clears these listeners, so the next
 * subscriber opens fresh scopes and watches again.
 */
function watchConnection() {
  if (stopWatchingConnection) return;
  let droppedAt: number | null = null;
  let previous: ConnectionState | null = null;
  const stopState = relayClient.subscribeToConnectionState((state) => {
    // The first call replays the current state; only a change counts.
    if (previous !== null && previous !== "idle" && state === "idle") {
      resetMeLiveBundle();
      return;
    }
    if (previous === "connected" && state !== "connected") {
      droppedAt = Math.floor(Date.now() / 1_000);
    }
    previous = state;
  });
  const stopReconnects = relayClient.subscribeToReconnects(() => {
    const since =
      (droppedAt ?? Math.floor(Date.now() / 1_000)) -
      RECONNECT_REPLAY_CHANNEL_LOOKBACK_SECS;
    droppedAt = null;
    void bundle.catchUp(since);
  });
  stopWatchingConnection = () => {
    stopState();
    stopReconnects();
  };
}

/**
 * `relayClient.subscribeLive` for a filter about the account: one pubkey in
 * `authors` or `#p`. Such filters share two REQs (see `MeLiveBundle`); any
 * other filter gets its own REQ as before. An object so tests can stub it.
 */
export const meLive = {
  subscribe(
    filter: RelaySubscriptionFilter,
    onEvent: (event: RelayEvent) => void,
    onReady?: (readiness: LiveSubscriptionReadiness) => void,
  ): Promise<() => Promise<void>> {
    if (!meScopeOf(filter)) {
      return relayClient.subscribeLive(filter, onEvent, onReady);
    }
    watchConnection();
    return bundle.subscribe(filter, onEvent, onReady);
  },
};

/** Drop every scope and stop watching (session torn down). */
export function resetMeLiveBundle() {
  bundle.reset();
  stopWatchingConnection?.();
  stopWatchingConnection = null;
}
