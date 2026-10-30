import type { QueryClient } from "@tanstack/react-query";

import { firstOpenWindowSettled } from "@/shared/api/openWindowRequests";

/** The channel list query (`channelsQueryKey` in features/channels/hooks.ts). */
const CHANNELS_KEY = ["channels"] as const;
/** After the channel list, how long the first room window may keep it waiting. */
export const ROOM_WINDOW_WAIT_MS = 2_000;
/** Agent indicators start by then even if the channel list never loads. */
export const STARTUP_WAIT_MS = 10_000;

let passed = false;
/** Bumped by a community switch; a run of an older one never passes this one. */
let generation = 0;
/** Ends the runs still waiting (timers, cache subscription). */
const pendingRuns = new Set<() => void>();

/**
 * A community switch waits again for its own channel list and room. Runs of
 * the old community stop and resolve without passing the new one.
 */
export function resetStartupObserverGate(): void {
  generation += 1;
  passed = false;
  for (const stop of [...pendingRuns]) stop();
}

/** The relay's channel list arrived (a stored snapshot has updatedAt 0) or failed. */
function channelsSettled(queryClient: QueryClient): boolean {
  const state = queryClient.getQueryState(CHANNELS_KEY);
  return (state?.dataUpdatedAt ?? 0) > 0 || state?.status === "error";
}

/**
 * Resolves once the launch's channel list and first room window have loaded
 * (docs/message-sync.md#launch-order), so the agent indicator replay (five
 * minutes of frames) never goes out ahead of them. With no room open, it
 * waits {@link ROOM_WINDOW_WAIT_MS} after the channel list; never longer
 * than {@link STARTUP_WAIT_MS}. Resolves at once after it passed.
 */
export function afterStartupWindows(
  queryClient: QueryClient,
  waits = { room: ROOM_WINDOW_WAIT_MS, total: STARTUP_WAIT_MS },
): Promise<void> {
  if (passed) return Promise.resolve();
  const runGeneration = generation;
  return new Promise((resolve) => {
    let roomSettled = false;
    let ended = false;
    let roomTimer: ReturnType<typeof setTimeout> | null = null;
    const stop = () => {
      if (ended) return;
      ended = true;
      pendingRuns.delete(stop);
      unsubscribe();
      clearTimeout(totalTimer);
      if (roomTimer !== null) clearTimeout(roomTimer);
      resolve();
    };
    const finish = () => {
      if (ended) return;
      if (runGeneration === generation) passed = true;
      stop();
    };
    const check = () => {
      if (ended) return;
      if (passed || (roomSettled && channelsSettled(queryClient))) {
        finish();
      } else if (channelsSettled(queryClient)) {
        roomTimer ??= setTimeout(finish, waits.room);
      }
    };
    const totalTimer = setTimeout(finish, waits.total);
    const unsubscribe = queryClient.getQueryCache().subscribe(check);
    pendingRuns.add(stop);
    void firstOpenWindowSettled().then(() => {
      roomSettled = true;
      check();
    });
    check();
  });
}
