import { invoke } from "@tauri-apps/api/core";

/**
 * Remove the connection from the native manager before waiting for its socket
 * task to stop. The command is bounded and idempotent; failures mean the
 * process is already tearing down or the socket is already gone.
 */
export function closeWebSocket(
  id: number,
  reason: string,
  invokeFn: typeof invoke = invoke,
): Promise<void> {
  return invokeFn("plugin:websocket|disconnect", { id }).then(
    () => undefined,
    (err) => {
      console.debug(`closeWebSocket(${id}, ${reason}) rejected:`, err);
    },
  );
}

export function closeAllWebSockets(
  invokeFn: typeof invoke = invoke,
): Promise<void> {
  return invokeFn("plugin:websocket|disconnect_all").then(
    () => undefined,
    (err) => {
      console.debug("closeAllWebSockets() rejected:", err);
    },
  );
}

/** How long a reload waits for native sockets to close before going ahead. */
export const RELOAD_TEARDOWN_TIMEOUT_MS = 500;

/**
 * Close every native socket, then reload. A webview reload does not close
 * them: the native read loop outlives the page and keeps the connection (and
 * its relay subscriptions) open. A close that hangs never blocks the reload.
 */
export async function closeSocketsThenReload(
  deps: {
    closeAll?: () => Promise<void>;
    reload?: () => void;
    timeoutMs?: number;
  } = {},
): Promise<void> {
  const {
    closeAll = () => closeAllWebSockets(),
    reload = () => window.location.reload(),
    timeoutMs = RELOAD_TEARDOWN_TIMEOUT_MS,
  } = deps;
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    await Promise.race([
      closeAll().catch(() => undefined),
      new Promise<void>((resolve) => {
        timer = setTimeout(resolve, timeoutMs);
      }),
    ]);
  } finally {
    if (timer !== undefined) clearTimeout(timer);
    reload();
  }
}
