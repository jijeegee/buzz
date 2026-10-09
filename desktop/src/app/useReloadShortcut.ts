import * as React from "react";

import { closeAllWebSockets } from "@/shared/api/relayWebSocketClose";
import { hasPrimaryShortcutModifier } from "@/shared/lib/platform";

const RELOAD_TEARDOWN_TIMEOUT_MS = 500;

/** Browser page-reload keys: Cmd/Ctrl+R, Cmd/Ctrl+Shift+R, and F5. */
export function isBrowserReloadKey(
  event: Pick<
    KeyboardEvent,
    "key" | "altKey" | "shiftKey" | "metaKey" | "ctrlKey"
  >,
): boolean {
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
 * Reload is a web-page gesture, not an app action, so installed builds swallow
 * the browser reload keys instead of letting the webview reload itself.
 * Development builds keep Cmd/Ctrl+R as a reload after bounded native
 * WebSocket teardown.
 */
export function useReloadShortcut() {
  React.useEffect(() => {
    async function handleKeyDown(event: KeyboardEvent) {
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

    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, []);
}
