import type { TimelineMessageDelta } from "@/features/messages/lib/timelineSnapshot";

export function getPinnedCenterDrift({
  contentTop,
  currentContentTop,
}: {
  contentTop: number;
  currentContentTop: number;
}): number | null {
  const drift = currentContentTop - contentTop;
  return Math.abs(drift) > 0.5 ? drift : null;
}

export function shouldIgnorePinnedCenterScroll({
  currentScrollTop,
  expectedScrollTop,
  isWritingScroll,
}: {
  currentScrollTop: number;
  expectedScrollTop: number | null;
  isWritingScroll: boolean;
}): boolean {
  return isWritingScroll || expectedScrollTop === currentScrollTop;
}

// Programmatic bottom pins require the physical floor, not merely the looser
// UI at-bottom threshold used for unread affordances.
const TRUE_BOTTOM_THRESHOLD_PX = 1;

type BottomSettleContainer = Pick<
  HTMLDivElement,
  "scrollHeight" | "clientHeight" | "scrollTop" | "scrollTo"
>;

// A pin that cannot move the view any further has reached the floor the
// browser allows. Fractional layout (display scaling, sub-pixel row heights)
// can leave that floor slightly more than 1px short of the computed bottom;
// insisting on the exact floor would keep the guard armed forever and snap
// every later user scroll back down.
const NEAR_BOTTOM_THRESHOLD_PX = 32;

export function settleProgrammaticBottomPin(
  container: BottomSettleContainer,
): boolean {
  const scrollTopBefore = container.scrollTop;
  container.scrollTo({ top: container.scrollHeight, behavior: "auto" });
  const gap =
    container.scrollHeight - container.clientHeight - container.scrollTop;
  if (gap <= TRUE_BOTTOM_THRESHOLD_PX) return true;
  return (
    container.scrollTop === scrollTopBefore && gap <= NEAR_BOTTOM_THRESHOLD_PX
  );
}

export function shouldSettleForSplitPanel({
  isAtBottom,
  splitPanelOpen,
}: {
  isAtBottom: boolean;
  splitPanelOpen: boolean;
}): boolean {
  return isAtBottom && splitPanelOpen;
}

export function shouldSettleVirtualizedBottom({
  isAtBottom,
  messageDelta,
  messagesArrived,
  messagesChanged,
}: {
  isAtBottom: boolean;
  messageDelta: TimelineMessageDelta;
  messagesArrived: number;
  messagesChanged: boolean;
}): boolean {
  return (
    isAtBottom &&
    messageDelta !== "prepend" &&
    (messagesArrived > 0 || messagesChanged)
  );
}
