import * as React from "react";

import type { ReadingPosition } from "@/features/channels/readingPositionStore";

/** Scrolling must rest this long before the position is reported. */
const REPORT_DELAY_MS = 300;
/** Within this distance of the floor the thread counts as at its newest. */
const AT_BOTTOM_PX = 32;

/**
 * Reports where the reader is in a thread once scrolling rests: the reply at
 * the top of the view and its distance from the scroller top, or null at the
 * newest reply (which forgets the position, as rooms do).
 */
export function useThreadReadingPositionReport(
  scrollContainerRef: React.RefObject<HTMLElement | null>,
  onChange: ((position: ReadingPosition | null) => void) | undefined,
  /** The thread head: not a reply, so never the position. */
  headId: string | null,
) {
  const onChangeRef = React.useRef(onChange);
  onChangeRef.current = onChange;
  const enabled = onChange !== undefined;
  React.useEffect(() => {
    const container = scrollContainerRef.current;
    if (!enabled || !container) return;
    let timer: number | undefined;
    const report = () => {
      const { scrollHeight, scrollTop, clientHeight } = container;
      if (scrollHeight - scrollTop - clientHeight <= AT_BOTTOM_PX) {
        onChangeRef.current?.(null);
        return;
      }
      const top = container.getBoundingClientRect().top;
      for (const row of container.querySelectorAll<HTMLElement>(
        "[data-message-id]",
      )) {
        const rect = row.getBoundingClientRect();
        const messageId = row.dataset.messageId;
        if (rect.bottom > top && messageId && messageId !== headId) {
          onChangeRef.current?.({ messageId, topPx: rect.top - top });
          return;
        }
      }
    };
    const handleScroll = () => {
      window.clearTimeout(timer);
      timer = window.setTimeout(report, REPORT_DELAY_MS);
    };
    container.addEventListener("scroll", handleScroll, { passive: true });
    return () => {
      window.clearTimeout(timer);
      container.removeEventListener("scroll", handleScroll);
    };
  }, [enabled, headId, scrollContainerRef]);
}
