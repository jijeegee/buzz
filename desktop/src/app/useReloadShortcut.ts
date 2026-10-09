import * as React from "react";

import { closeAllWebSockets } from "@/shared/api/relayWebSocketClose";
import { hasPrimaryShortcutModifier } from "@/shared/lib/platform";

const RELOAD_TEARDOWN_TIMEOUT_MS = 500;

type ShortcutKeyEvent = Pick<
  KeyboardEvent,
  "key" | "altKey" | "shiftKey" | "metaKey" | "ctrlKey"
>;

/** Browser page-reload keys: Cmd/Ctrl+R, Cmd/Ctrl+Shift+R, and F5. */
export function isBrowserReloadKey(event: ShortcutKeyEvent): boolean {
  if (event.key === "F5") {
    return !event.altKey;
  }
  return (
    hasPrimaryShortcutModifier(event) &&
    !event.altKey &&
    event.key.toLowerCase() === "r"
  );
}

/**
 * Other built-in webview shortcuts that open browser UI: find on page
 * (Cmd/Ctrl+F, Cmd/Ctrl+G, F3), print (Cmd/Ctrl+P), and caret browsing (F7).
 * The app binds some of these itself, so they are only swallowed when no app
 * handler claimed the event.
 */
export function isBrowserUiKey(event: ShortcutKeyEvent): boolean {
  if (event.altKey) return false;
  if (event.key === "F3" || event.key === "F7") return true;
  if (!hasPrimaryShortcutModifier(event)) return false;
  const key = event.key.toLowerCase();
  return key === "f" || key === "g" || key === "p";
}

/**
 * Reload and other browser UI are web-page gestures, not app actions, so
 * installed builds swallow those webview shortcuts. Development builds keep
 * Cmd/Ctrl+R as a reload after bounded native WebSocket teardown.
 */
export function useReloadShortcut() {
  React.useEffect(() => {
    async function handleKeyDown(event: KeyboardEvent) {
      if (isBrowserUiKey(event)) {
        // Added during capture, so it runs last in the bubble phase on
        // window, after every app keydown handler had its chance. If a
        // handler stops propagation it never runs and the next keydown
        // removes it without touching that unrelated event.
        const swallowIfUnhandled = (later: KeyboardEvent) => {
          if (later === event && !later.defaultPrevented) {
            later.preventDefault();
          }
        };
        window.addEventListener("keydown", swallowIfUnhandled, { once: true });
        return;
      }
      if (!isBrowserReloadKey(event)) {
        return;
      }

      event.preventDefault();
      if (!import.meta.env.DEV || event.key === "F5" || event.shiftKey) {
        return;
      }

      await Promise.race([
        closeAllWebSockets(),
        new Promise<void>((resolve) =>
          window.setTimeout(resolve, RELOAD_TEARDOWN_TIMEOUT_MS),
        ),
      ]);
      window.location.reload();
    }

    window.addEventListener("keydown", handleKeyDown, { capture: true });
    return () =>
      window.removeEventListener("keydown", handleKeyDown, { capture: true });
  }, []);
}
