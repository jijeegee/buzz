import * as React from "react";

import { meLive } from "@/shared/api/relayMeLive";
import { KIND_NEW_SIGNIN_NOTICE } from "@/shared/constants/kinds";
import { parseNewSignInNotice, type NewSignInNotice } from "./newSignInNotice";

const RETRY_BASE_MS = 2_000;
const RETRY_MAX_MS = 60_000;

/**
 * The latest "new sign-in" notice the relay pushed for `pubkey` while this
 * app was open (kind 24211, live only), and a way to dismiss it.
 */
export function useNewSignInNotice(pubkey: string | undefined): {
  notice: NewSignInNotice | null;
  dismiss: () => void;
} {
  const me = pubkey?.trim().toLowerCase() ?? "";
  const [notice, setNotice] = React.useState<NewSignInNotice | null>(null);

  React.useEffect(() => {
    setNotice(null);
    if (me.length === 0) return;
    let cancelled = false;
    let dispose: (() => Promise<void>) | null = null;
    let retryTimer: number | null = null;
    let attempt = 0;
    const start = () => {
      retryTimer = null;
      meLive
        .subscribe(
          { kinds: [KIND_NEW_SIGNIN_NOTICE], "#p": [me], limit: 0 },
          (event) => {
            const parsed = parseNewSignInNotice(event, me);
            if (parsed && !cancelled) setNotice(parsed);
          },
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
      if (retryTimer !== null) window.clearTimeout(retryTimer);
      if (dispose) void dispose();
    };
  }, [me]);

  const dismiss = React.useCallback(() => setNotice(null), []);
  return { notice, dismiss };
}
