import * as React from "react";

import { meLive } from "@/shared/api/relayMeLive";
import { KIND_CHAT_STATE_PING } from "@/shared/constants/kinds";
import { createTrailingDebounce } from "@/shared/lib/trailingDebounce";

/** A burst of pings (one per write) collapses into one refetch. */
export const CHAT_STATE_PING_DEBOUNCE_MS = 300;
const RETRY_BASE_MS = 2_000;
const RETRY_MAX_MS = 60_000;

/**
 * Listen for the relay's kind:24210 ping, sent to all of the user's devices
 * after any change to their chat state, and call `onPing` (debounced).
 */
export function useChatStatePing(
  pubkey: string | undefined,
  enabled: boolean,
  onPing: () => void,
): void {
  const handlePing = React.useEffectEvent(onPing);
  const me = pubkey?.trim().toLowerCase() ?? "";

  React.useEffect(() => {
    if (!enabled || me.length === 0) return;
    let cancelled = false;
    let dispose: (() => Promise<void>) | null = null;
    let retryTimer: number | null = null;
    let attempt = 0;
    const debounce = createTrailingDebounce(
      () => handlePing(),
      CHAT_STATE_PING_DEBOUNCE_MS,
    );
    const start = () => {
      retryTimer = null;
      meLive
        .subscribe(
          { kinds: [KIND_CHAT_STATE_PING], "#p": [me], limit: 0 },
          () => debounce.trigger(),
        )
        .then((unsubscribe) => {
          if (cancelled) {
            void unsubscribe();
            return;
          }
          attempt = 0;
          dispose = unsubscribe;
        })
        .catch(() => {
          if (cancelled) return;
          const delay = Math.min(RETRY_MAX_MS, RETRY_BASE_MS * 2 ** attempt);
          attempt = Math.min(attempt + 1, 5);
          retryTimer = window.setTimeout(start, delay);
        });
    };
    start();
    return () => {
      cancelled = true;
      debounce.cancel();
      if (retryTimer !== null) window.clearTimeout(retryTimer);
      if (dispose) void dispose();
    };
  }, [enabled, me]);
}
