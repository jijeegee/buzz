import * as React from "react";

/**
 * A request to land a channel's main timeline on its newest message, made when
 * the inbox opens a channel row. It only scrolls the already-mounted timeline;
 * opening the channel itself is ordinary navigation.
 */
type LandAtLatestRequest = { channelId: string; seq: number; at: number };

/** A request the channel never picked up (a failed open) expires. */
const REQUEST_TTL_MS = 10_000;

const listeners = new Set<() => void>();

let request: LandAtLatestRequest | null = null;
/** Requests are consumed once, so a later visit restores position as usual. */
let handledSeq = 0;

export function requestLandAtLatest(channelId: string): void {
  request = { channelId, seq: (request?.seq ?? 0) + 1, at: Date.now() };
  for (const listener of listeners) {
    listener();
  }
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** Frames to keep the timeline pinned while late layout and history settle. */
const MAX_LAND_FRAMES = 120;
const SETTLED_FRAMES = 15;

/**
 * Pins `channelId`'s timeline to its bottom once per request, retrying each
 * frame until the timeline can settle and then briefly holding it there.
 */
export function useLandAtLatest(
  channelId: string | null,
  settleAtBottom: () => boolean,
) {
  const current = React.useSyncExternalStore(
    subscribe,
    () => request,
    () => null,
  );
  const settleRef = React.useRef(settleAtBottom);
  settleRef.current = settleAtBottom;
  const frameRef = React.useRef(0);

  // A re-render must not cut a landing short; only a channel change or
  // unmount stops it.
  // biome-ignore lint/correctness/useExhaustiveDependencies: stop a landing when the channel changes
  React.useEffect(
    () => () => window.cancelAnimationFrame(frameRef.current),
    [channelId],
  );

  React.useEffect(() => {
    if (
      current === null ||
      current.channelId !== channelId ||
      current.seq <= handledSeq ||
      Date.now() - current.at > REQUEST_TTL_MS
    ) {
      return;
    }
    handledSeq = current.seq;
    window.cancelAnimationFrame(frameRef.current);
    let frame = 0;
    let settledFrames = 0;
    const step = () => {
      frame += 1;
      if (settleRef.current()) settledFrames += 1;
      if (frame >= MAX_LAND_FRAMES || settledFrames >= SETTLED_FRAMES) return;
      frameRef.current = window.requestAnimationFrame(step);
    };
    frameRef.current = window.requestAnimationFrame(step);
  }, [channelId, current]);
}
