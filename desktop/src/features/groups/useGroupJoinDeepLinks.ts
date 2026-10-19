import * as React from "react";

import { listenForGroupJoinDeepLinks } from "@/shared/deep-link";
import { openJoinWithLink } from "./joinWithLinkStore";

/**
 * `kuzz://join/<code>` and `kuzz://join/@<handle>` opened from the browser or
 * another app: show Join with link with the link filled in and looked up.
 */
export function useGroupJoinDeepLinks(enabled = true) {
  React.useEffect(() => {
    if (!enabled) return;
    let cancelled = false;
    const unlistenPromise = listenForGroupJoinDeepLinks((href) => {
      if (cancelled) return false;
      openJoinWithLink(href);
      return true;
    });
    return () => {
      cancelled = true;
      void unlistenPromise.then((fn) => fn());
    };
  }, [enabled]);
}
