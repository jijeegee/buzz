/**
 * The refresh after a room's live subscription is set up asks the relay for
 * the newest window again, to close the gap between that window and the
 * listener going live. It is skipped when it is provably redundant: the
 * window request this open made went out while the open's room listener was
 * attached and a live REQ carrying the room had reached EOSE, and neither has
 * changed since (docs/message-sync.md#open-room-and-thread-requests).
 */
import type { QueryClient } from "@tanstack/react-query";

import {
  type StoreLiveCoverage,
  storeLiveCoverage,
} from "@/shared/api/relayStoreCoverage";
import type { RelayEvent } from "@/shared/api/types";

/** One open's room listener; the function that stops it identifies it. */
export type RoomWindowListener = () => Promise<void>;

type RoomListenSource = {
  subscribeToChannelLive: (
    roomId: string,
    onEvent: (event: RelayEvent) => void,
  ) => Promise<() => Promise<void>>;
};

/** What vouched for a window request when it went out. */
type LiveToken = {
  listeners: ReadonlySet<RoomWindowListener>;
  coverage: readonly StoreLiveCoverage[];
};

type WindowSend = { token: LiveToken | null; committed: boolean };

/** Room listeners attached now, per room (attached from the listen call). */
const attached = new Map<string, Set<RoomWindowListener>>();
/** The last newest-window request per room, per query cache. */
const lastSends = new WeakMap<QueryClient, Map<string, WindowSend>>();
/** Window answers by their request, until they are committed. */
const answers = new WeakMap<readonly RelayEvent[], WindowSend>();

/**
 * Listen to one room's live events. The listener counts as attached from
 * this call (the room bundle delivers to it at once), until it is stopped.
 */
export function listenRoomWindow(
  source: RoomListenSource,
  roomId: string,
  onEvent: (event: RelayEvent) => void,
): Promise<RoomWindowListener> {
  const started = source.subscribeToChannelLive(roomId, onEvent);
  const detach = () => {
    const listeners = attached.get(roomId);
    listeners?.delete(stop);
    if (listeners?.size === 0) attached.delete(roomId);
  };
  const stop: RoomWindowListener = async () => {
    detach();
    await (await started)();
  };
  let listeners = attached.get(roomId);
  if (!listeners) {
    listeners = new Set();
    attached.set(roomId, listeners);
  }
  listeners.add(stop);
  return started.then(
    () => stop,
    (error: unknown) => {
      detach();
      throw error;
    },
  );
}

function captureToken(roomId: string): LiveToken | null {
  const listeners = attached.get(roomId);
  const coverage = storeLiveCoverage(roomId);
  if (!listeners?.size || coverage.length === 0) return null;
  return { listeners: new Set(listeners), coverage };
}

/**
 * Ask for `roomId`'s newest window through `request`, noting the live token
 * as it goes out. Taken before the request is dispatched, so the token can
 * only be older than the send.
 */
export async function sendRoomWindow(
  queryClient: QueryClient,
  roomId: string,
  request: () => Promise<RelayEvent[]>,
): Promise<RelayEvent[]> {
  const send: WindowSend = { token: captureToken(roomId), committed: false };
  let sends = lastSends.get(queryClient);
  if (!sends) {
    sends = new Map();
    lastSends.set(queryClient, sends);
  }
  sends.set(roomId, send);
  const events = await request();
  answers.set(events, send);
  return events;
}

/** A fetched window became the room's newest window. */
export function noteRoomWindowCommitted(events: readonly RelayEvent[]) {
  const send = answers.get(events);
  if (send) send.committed = true;
}

/**
 * The token still vouches for `listener`: it was attached when the request
 * went out and still is, and every live REQ that carried the room then has
 * stayed live without a gap since.
 */
function holds(send: WindowSend, roomId: string, listener: RoomWindowListener) {
  const token = send.token;
  return (
    token?.listeners.has(listener) === true &&
    attached.get(roomId)?.has(listener) === true &&
    token.coverage.every(
      ({ subscription, coverage }) => subscription.storeCoverage === coverage,
    )
  );
}

/**
 * Whether the room's newest window was fetched under a live token that still
 * holds for `listener`, so a refresh would only repeat it. The room query's
 * fetch still running (`inFlight`) is waited for first, so its request is the
 * one judged even if it had not gone out yet, but only when a token could
 * hold (listener attached, room live now): otherwise the refresh joins that
 * fetch at once. A failed or replaced request is not proof.
 */
export async function windowFetchedLive(
  queryClient: QueryClient,
  roomId: string,
  listener: RoomWindowListener,
  inFlight: Promise<unknown> | undefined,
): Promise<boolean> {
  if (inFlight) {
    const couldHold =
      attached.get(roomId)?.has(listener) === true &&
      storeLiveCoverage(roomId).length > 0;
    if (!couldHold) return false;
    await inFlight.catch(() => {});
  }
  const send = lastSends.get(queryClient)?.get(roomId);
  return send?.committed === true && holds(send, roomId, listener);
}
