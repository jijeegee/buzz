import * as React from "react";

import {
  getCachedMediaProxyPort,
  getMediaProxyState,
  subscribeMediaProxyPort,
} from "./mediaUrl";

/**
 * The resolved localhost media-proxy port, re-rendering when it resolves or
 * changes. Components that call `rewriteRelayUrl` during render can subscribe
 * to this so an initial `kuzz-media://` fallback is replaced by the loopback
 * proxy URL as soon as the Tauri backend reports the port.
 */
export function useMediaProxyPort(): number | null {
  return React.useSyncExternalStore(
    subscribeMediaProxyPort,
    getCachedMediaProxyPort,
    () => null,
  );
}

/**
 * Whether a relay image should wait for the proxy port: true while the port
 * lookup is still running, so the image is requested once, from the cacheable
 * loopback URL, instead of first through the uncached `kuzz-media://` path.
 */
export function useAwaitingMediaProxy(): boolean {
  return (
    React.useSyncExternalStore(
      subscribeMediaProxyPort,
      getMediaProxyState,
      () => -1,
    ) === 0
  );
}
