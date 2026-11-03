import type { RelaySubscriptionFilter } from "@/shared/api/relayClientShared";
import { chunkRoomIds } from "@/shared/api/relayRoomBundle";
import type { RelayEvent } from "@/shared/api/types";
import { KIND_STREAM_MESSAGE } from "@/shared/constants/kinds";
import { normalizePubkey } from "@/shared/lib/pubkey";
import { REMOTE_START_MAX_AGE_SECONDS } from "./remoteStartCommand";

/** How many times an added DM's read waits again for late coverage. */
export const REMOTE_START_COVERAGE_RETRIES = 3;

type Listener = (event: RelayEvent) => void;

export type RemoteStartWatchDeps = {
  ownerPubkey: string;
  /** The huddle window carries no rooms and acts on no command. */
  inHuddleWindow: boolean;
  /** The room bundle's listed (member) rooms. */
  listenAllRooms: (listener: Listener) => () => void;
  /** The room bundle's quiet rooms (hidden DMs). */
  listenQuietRooms: (listener: Listener) => () => void;
  whenRoomsCovered: (roomIds: readonly string[]) => Promise<boolean>;
  fetchEvents: (filter: RelaySubscriptionFilter) => Promise<RelayEvent[]>;
  /** Whether a quiet room is the owner's DM with one of this computer's
   *  local agents (checked against the agents at the time of the event). */
  isAgentDm: (roomId: string) => Promise<boolean>;
  /** Acts on one of the owner's messages (`handleRemoteCommand`): its ledger
   *  acts on an event once, however many paths carry it. */
  handle: (event: RelayEvent) => void;
  nowSeconds: () => number;
};

export type RemoteStartWatch = {
  /** The owner's listed DMs with local agents. */
  setWanted: (roomIds: readonly string[]) => void;
  stop: () => void;
};

function roomOf(event: RelayEvent) {
  return event.tags.find((tag) => tag[0] === "h")?.[1] ?? null;
}

/**
 * Hears the owner's Start / Stop commands from another device on the room
 * bundle instead of a REQ of its own: listed agent DMs through the all-rooms
 * listeners, hidden agent DMs through the quiet listeners. Missed commands
 * come back through the bundle's reconnect repair.
 *
 * A DM listed after the first set may have been created by the command
 * itself, before its chunk was live: once its chunk is covered, its last
 * `REMOTE_START_MAX_AGE_SECONDS` are read back once (again on late coverage).
 * The first set reads nothing: a command sent before launch is not acted on.
 */
export function startRemoteStartWatch(
  deps: RemoteStartWatchDeps,
): RemoteStartWatch {
  if (deps.inHuddleWindow) return { setWanted: () => {}, stop: () => {} };
  const owner = normalizePubkey(deps.ownerPubkey);
  let wanted: ReadonlySet<string> = new Set();
  let baselined = false;
  let stopped = false;

  const isOwnerCommand = (event: RelayEvent) =>
    event.kind === KIND_STREAM_MESSAGE &&
    normalizePubkey(event.pubkey) === owner &&
    event.content.trim().startsWith("!");

  const stopListed = deps.listenAllRooms((event) => {
    const roomId = roomOf(event);
    if (!roomId || !wanted.has(roomId) || !isOwnerCommand(event)) return;
    deps.handle(event);
  });
  const stopQuiet = deps.listenQuietRooms((event) => {
    const roomId = roomOf(event);
    if (!roomId || !isOwnerCommand(event)) return;
    void deps.isAgentDm(roomId).then(
      (agentDm) => {
        if (agentDm && !stopped) deps.handle(event);
      },
      (error: unknown) => {
        console.error("Remote start: could not check the DM", roomId, error);
      },
    );
  });

  const readBack = async (roomIds: readonly string[]) => {
    const since = deps.nowSeconds() - REMOTE_START_MAX_AGE_SECONDS;
    for (const chunk of chunkRoomIds(roomIds)) {
      const events = await deps.fetchEvents({
        kinds: [KIND_STREAM_MESSAGE],
        authors: [deps.ownerPubkey],
        "#h": chunk,
        since,
        limit: Math.min(20 * chunk.length, 500),
      });
      if (stopped) return;
      for (const event of events) {
        if (isOwnerCommand(event)) deps.handle(event);
      }
    }
  };

  const readAdded = async (added: readonly string[]) => {
    let covered = await deps.whenRoomsCovered(added);
    if (stopped) return;
    await readBack(added);
    for (
      let attempt = 0;
      !covered && attempt < REMOTE_START_COVERAGE_RETRIES && !stopped;
      attempt += 1
    ) {
      covered = await deps.whenRoomsCovered(added);
      if (covered && !stopped) await readBack(added);
    }
  };

  return {
    setWanted(roomIds) {
      if (stopped) return;
      const next = new Set(roomIds);
      const previous = wanted;
      wanted = next;
      // The first non-empty set is the baseline (agents and DMs loaded).
      if (!baselined) {
        baselined = next.size > 0;
        return;
      }
      const added = [...next].filter((roomId) => !previous.has(roomId));
      if (added.length === 0) return;
      readAdded(added).catch((error: unknown) => {
        console.error("Remote start: could not read added DMs", error);
      });
    },
    stop() {
      stopped = true;
      stopListed();
      stopQuiet();
    },
  };
}
