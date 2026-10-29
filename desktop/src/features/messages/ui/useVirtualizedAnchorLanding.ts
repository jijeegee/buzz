import * as React from "react";
import type { VListHandle } from "virtua";

/** Gap between the top chrome and the row a list opens at. */
const ANCHOR_TOP_GAP_PX = 8;
/** Frames the anchor must hold still before the list is shown. */
const ANCHOR_STABLE_FRAMES = 2;
/** Upper bound on hidden frames, so a list that never settles still shows. */
const ANCHOR_MAX_FRAMES = 30;

/**
 * Opens the list at its final position without a visible move: the list
 * stays hidden while Virtua measures the rows around where it opens, then is
 * shown once that position holds still. `land` re-aims at a row every frame,
 * finding its index again so rows loaded above it while hidden do not move it:
 * just under the top chrome, `topPx` from the scroller top when reopening a
 * remembered position, or centred for a linked message. `landAtBottom` lets
 * the bottom settle (which keeps following the floor after the list is shown)
 * pin the newest row. Reports the real bottom state when shown, since a short
 * unread run clamps to the floor.
 */
export function useVirtualizedAnchorLanding(
  hostRef: React.RefObject<HTMLDivElement | null>,
  listRef: React.RefObject<VListHandle | null>,
  onAtBottomStateChangeRef: React.RefObject<
    ((atBottom: boolean) => void) | undefined
  >,
) {
  const frameRef = React.useRef<number | null>(null);
  const landingIndexRef = React.useRef<(() => number) | null>(null);

  const reveal = React.useCallback(() => {
    if (frameRef.current !== null) {
      cancelAnimationFrame(frameRef.current);
      frameRef.current = null;
    }
    landingIndexRef.current = null;
    hostRef.current?.style.removeProperty("visibility");
  }, [hostRef]);

  /** Hides the list and shows it once `position` holds still. */
  const hold = React.useCallback(
    (list: VListHandle, position: () => number[], aim?: () => void) => {
      if (frameRef.current !== null) cancelAnimationFrame(frameRef.current);
      let frame = 0;
      let stableFrames = 0;
      let last: number[] = [];
      const step = () => {
        frameRef.current = null;
        frame += 1;
        const current = position();
        const still = current.every(
          (value, index) => Math.abs(value - (last[index] ?? Number.NaN)) < 0.5,
        );
        stableFrames = still ? stableFrames + 1 : 0;
        last = current;
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
        aim?.();
        frameRef.current = requestAnimationFrame(step);
      };
      frameRef.current = requestAnimationFrame(step);
    },
    [onAtBottomStateChangeRef, reveal],
  );

  const land = React.useCallback(
    (
      indexOf: () => number,
      topPx: number | null = null,
      align: "start" | "center" = "start",
    ) => {
      const host = hostRef.current;
      const list = listRef.current;
      const scroller = host?.firstElementChild;
      let found = indexOf();
      if (!host || !list || !(scroller instanceof HTMLDivElement) || found < 0)
        return false;
      const current = () => {
        const next = indexOf();
        if (next >= 0) found = next;
        return found;
      };
      // Virtua's start alignment already clears the scroller's top padding
      // (the space under the top chrome).
      const aim = () => {
        const index = current();
        if (align === "center") list.scrollToIndex(index, { align: "center" });
        else if (topPx === null)
          list.scrollToIndex(index, {
            align: "start",
            offset: -ANCHOR_TOP_GAP_PX,
          });
        else list.scrollTo(list.getItemOffset(index) - topPx);
      };
      host.style.visibility = "hidden";
      landingIndexRef.current = current;
      aim();
      hold(
        list,
        () => [list.getItemOffset(current()) - list.scrollOffset],
        aim,
      );
      return true;
    },
    [hold, hostRef, listRef],
  );

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
      // The settle's own resize observer re-pins as rows are measured; the
      // list is shown once the floor and the offset stop moving.
      hold(list, () => [list.scrollSize, list.scrollOffset]);
    },
    [hold, hostRef, listRef],
  );

  const isLandingAt = React.useCallback(
    (index: number) => landingIndexRef.current?.() === index,
    [],
  );

  React.useEffect(() => reveal, [reveal]);
  return { land, landAtBottom, isLandingAt, cancel: reveal };
}
