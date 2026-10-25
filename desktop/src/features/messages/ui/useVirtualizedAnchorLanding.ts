import * as React from "react";
import type { VListHandle } from "virtua";

/** Gap between the top chrome and the row a list opens at. */
const ANCHOR_TOP_GAP_PX = 8;
/** Frames the anchor must hold still before the list is shown. */
const ANCHOR_STABLE_FRAMES = 2;
/** Upper bound on hidden frames, so a list that never settles still shows. */
const ANCHOR_MAX_FRAMES = 30;

/**
 * Opens the list at a row (or the bottom) without a visible move: the
 * list stays hidden while Virtua measures the rows around the anchor and is
 * re-aimed every frame, then shown once the anchor holds still. The row goes
 * just under the top chrome, or `topPx` from the scroller top when reopening
 * a remembered position. Reports the real bottom state when shown, since a
 * short unread run clamps to the floor.
 */
export function useVirtualizedAnchorLanding(
  hostRef: React.RefObject<HTMLDivElement | null>,
  listRef: React.RefObject<VListHandle | null>,
  onAtBottomStateChangeRef: React.RefObject<
    ((atBottom: boolean) => void) | undefined
  >,
) {
  const frameRef = React.useRef<number | null>(null);

  const reveal = React.useCallback(() => {
    if (frameRef.current !== null) {
      cancelAnimationFrame(frameRef.current);
      frameRef.current = null;
    }
    hostRef.current?.style.removeProperty("visibility");
  }, [hostRef]);

  const land = React.useCallback(
    (index: number, topPx: number | null = null) => {
      const host = hostRef.current;
      const list = listRef.current;
      const scroller = host?.firstElementChild;
      if (!host || !list || !(scroller instanceof HTMLDivElement)) return false;
      // Virtua's start alignment already clears the scroller's top padding
      // (the space under the top chrome).
      const aim = () =>
        topPx === null
          ? list.scrollToIndex(index, {
              align: "start",
              offset: -ANCHOR_TOP_GAP_PX,
            })
          : list.scrollTo(list.getItemOffset(index) - topPx);
      host.style.visibility = "hidden";
      aim();
      let frame = 0;
      let stableFrames = 0;
      let lastTop = Number.NaN;
      const step = () => {
        frameRef.current = null;
        frame += 1;
        const top = list.getItemOffset(index) - list.scrollOffset;
        stableFrames = Math.abs(top - lastTop) < 0.5 ? stableFrames + 1 : 0;
        lastTop = top;
        if (
          stableFrames >= ANCHOR_STABLE_FRAMES ||
          frame >= ANCHOR_MAX_FRAMES
        ) {
          reveal();
          onAtBottomStateChangeRef.current?.(
            list.scrollSize - list.viewportSize - list.scrollOffset <= 32,
          );
          return;
        }
        aim();
        frameRef.current = requestAnimationFrame(step);
      };
      frameRef.current = requestAnimationFrame(step);
      return true;
    },
    [hostRef, listRef, onAtBottomStateChangeRef, reveal],
  );

  /**
   * Opens the list at the bottom the same way: hidden while `settle` pins it
   * to the floor every frame (rows measure in, the composer padding lands),
   * shown once the floor holds still. The first visible frame is final.
   */
  const landAtBottom = React.useCallback(
    (settle: () => void) => {
      const host = hostRef.current;
      const list = listRef.current;
      if (!host || !list) {
        settle();
        return;
      }
      host.style.visibility = "hidden";
      settle();
      let frame = 0;
      let stableFrames = 0;
      let lastSize = Number.NaN;
      let lastOffset = Number.NaN;
      const step = () => {
        frameRef.current = null;
        frame += 1;
        const size = list.scrollSize;
        const offset = list.scrollOffset;
        stableFrames =
          Math.abs(size - lastSize) < 0.5 && Math.abs(offset - lastOffset) < 0.5
            ? stableFrames + 1
            : 0;
        lastSize = size;
        lastOffset = offset;
        if (
          stableFrames >= ANCHOR_STABLE_FRAMES ||
          frame >= ANCHOR_MAX_FRAMES
        ) {
          reveal();
          onAtBottomStateChangeRef.current?.(true);
          return;
        }
        settle();
        frameRef.current = requestAnimationFrame(step);
      };
      frameRef.current = requestAnimationFrame(step);
    },
    [hostRef, listRef, onAtBottomStateChangeRef, reveal],
  );

  return { land, landAtBottom, cancel: reveal };
}
