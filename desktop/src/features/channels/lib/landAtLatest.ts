import * as React from "react";

/**
 * Set by a surface that opens a channel "as a chat room" (the inbox): the main
 * timeline lands on the channel's newest message instead of restoring an older
 * reading position.
 */
const LandAtLatestContext = React.createContext(false);

export const LandAtLatestProvider = LandAtLatestContext.Provider;

export function useLandAtLatest(): boolean {
  return React.useContext(LandAtLatestContext);
}

/** Frames to keep the timeline pinned while late layout and history settle. */
const MAX_LAND_FRAMES = 120;
const SETTLED_FRAMES = 15;

/**
 * Pins the timeline to its bottom once per mount when `active`, retrying each
 * frame until the timeline can settle and then briefly holding it there.
 */
export function useLandAtLatestOnMount(
  active: boolean,
  settleAtBottom: () => boolean,
) {
  const settleRef = React.useRef(settleAtBottom);
  settleRef.current = settleAtBottom;

  React.useEffect(() => {
    if (!active) return;
    let frame = 0;
    let settledFrames = 0;
    let handle = 0;
    const step = () => {
      frame += 1;
      if (settleRef.current()) settledFrames += 1;
      if (frame >= MAX_LAND_FRAMES || settledFrames >= SETTLED_FRAMES) return;
      handle = window.requestAnimationFrame(step);
    };
    handle = window.requestAnimationFrame(step);
    return () => window.cancelAnimationFrame(handle);
  }, [active]);
}
