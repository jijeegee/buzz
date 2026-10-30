import * as React from "react";

/** Keys that scroll a conversation. */
const SCROLL_KEYS = new Set([
  "ArrowUp",
  "ArrowDown",
  "PageUp",
  "PageDown",
  "Home",
  "End",
  " ",
]);

/**
 * Scrolls of a message list this soon after watching starts are the list
 * placing itself at the open, not the reader.
 */
export const READER_SCROLL_GRACE_MS = 750;

function isEditable(target: EventTarget | null) {
  return (
    target instanceof HTMLElement &&
    (target.isContentEditable ||
      target.tagName === "INPUT" ||
      target.tagName === "TEXTAREA")
  );
}

/** Find in page (Ctrl/Cmd+F, F3, Ctrl/Cmd+G) jumps to its matches. */
function isFindKey(event: KeyboardEvent) {
  const key = event.key.toLowerCase();
  return (
    key === "f3" || ((event.ctrlKey || event.metaKey) && /^[fg]$/.test(key))
  );
}

/** A scrolled element that holds message rows (a timeline viewport). */
function isMessageList(target: EventTarget | null) {
  return (
    target instanceof Element &&
    target.querySelector("[data-message-id]") !== null
  );
}

/**
 * Notes in `moved` whether the reader has moved the conversation since
 * `watching` turned on, or since `visit` changed: a wheel or touch scroll, a
 * scroll key outside a text field, a pointer press outside one (scrollbar
 * drag, jump buttons, autoscroll), find in page, or a scroll of a message
 * list after the list has placed itself. A provisional open moves to the
 * relay's position only while it stays false
 * (docs/message-sync.md#opening-at-a-position). Called after the render's
 * decision, so a decision made in this render is watched.
 */
export function useReaderMoved(
  moved: React.RefObject<boolean>,
  watching: boolean,
  visit: string | null,
) {
  const visitRef = React.useRef(visit);
  if (visitRef.current !== visit) {
    visitRef.current = visit;
    moved.current = false;
  }
  // biome-ignore lint/correctness/useExhaustiveDependencies: a new visit starts a new watch
  React.useEffect(() => {
    if (!watching) return;
    moved.current = false;
    const since = Date.now();
    const note = () => {
      moved.current = true;
    };
    const noteKey = (event: KeyboardEvent) => {
      if (isFindKey(event)) note();
      else if (SCROLL_KEYS.has(event.key) && !isEditable(event.target)) note();
    };
    const notePointer = (event: PointerEvent) => {
      if (!isEditable(event.target)) note();
    };
    const noteScroll = (event: Event) => {
      if (Date.now() - since >= READER_SCROLL_GRACE_MS) {
        if (isMessageList(event.target)) note();
      }
    };
    const options = { capture: true, passive: true };
    window.addEventListener("wheel", note, options);
    window.addEventListener("touchmove", note, options);
    window.addEventListener("pointerdown", notePointer, options);
    window.addEventListener("keydown", noteKey, true);
    document.addEventListener("scroll", noteScroll, options);
    return () => {
      window.removeEventListener("wheel", note, options);
      window.removeEventListener("touchmove", note, options);
      window.removeEventListener("pointerdown", notePointer, options);
      window.removeEventListener("keydown", noteKey, true);
      document.removeEventListener("scroll", noteScroll, options);
    };
  }, [moved, watching, visit]);
}
