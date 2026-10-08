import * as React from "react";

/**
 * Set by a surface that opens a channel "as a chat room" (the inbox): the main
 * timeline lands on the channel's newest message instead of restoring an older
 * reading position. The value identifies the entry (null when none), so each
 * new entry lands again without remounting the chat screen.
 */
const LandAtLatestContext = React.createContext<number | null>(null);

export const LandAtLatestProvider = LandAtLatestContext.Provider;

export function useLandAtLatest(): number | null {
  return React.useContext(LandAtLatestContext);
}

/** Frames to keep the timeline pinned while late layout and history settle. */
const MAX_LAND_FRAMES = 120;
const SETTLED_FRAMES = 15;

/**
 * Pins the timeline to its bottom once per entry, retrying each frame until the
 * timeline can settle and then briefly holding it there.
 */
export function useLandAtLatestOnEntry(
  entry: number | null,
  settleAtBottom: () => boolean,
) {
  const settleRef = React.useRef(settleAtBottom);
  settleRef.current = settleAtBottom;

  React.useEffect(() => {
    if (entry === null) return;
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
  }, [entry]);
}
