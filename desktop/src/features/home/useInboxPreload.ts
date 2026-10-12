import { useQueryClient } from "@tanstack/react-query";
import * as React from "react";

import type { InboxItem } from "@/features/home/lib/inbox";
import {
  INBOX_PRELOAD_MESSAGES,
  inboxPreloadTargetKey,
  inboxPreloadTargets,
} from "@/features/home/lib/inboxPreload";
import {
  preloadChannelMessages,
  preloadThreadReplies,
} from "@/features/messages/channelPreload";
import { isDocumentVisible } from "@/shared/lib/useDocumentVisible";

/** Input this recent means the user is typing or clicking: yield to them. */
const RECENT_INPUT_MS = 1_500;
const IDLE_POLL_MS = 500;
/** A target that failed (or was skipped) is not retried sooner than this. */
const RETRY_AFTER_MS = 5 * 60 * 1_000;

/**
 * Background-loads the rooms behind the top chat-list rows, one at a time,
 * so opening them paints at once. Pauses while the user types or clicks,
 * while a room is loading in the foreground, and while the app is hidden.
 */
export function useInboxPreload(items: readonly InboxItem[], enabled: boolean) {
  const queryClient = useQueryClient();
  const lastInputAtRef = React.useRef(0);
  const attemptedAtRef = React.useRef(new Map<string, number>());

  React.useEffect(() => {
    if (!enabled) return;
    const markInput = () => {
      lastInputAtRef.current = Date.now();
    };
    document.addEventListener("keydown", markInput, true);
    document.addEventListener("pointerdown", markInput, true);
    return () => {
      document.removeEventListener("keydown", markInput, true);
      document.removeEventListener("pointerdown", markInput, true);
    };
  }, [enabled]);

  const targets = React.useMemo(() => inboxPreloadTargets(items), [items]);
  const targetsKey = targets.map(inboxPreloadTargetKey).join("|");

  // biome-ignore lint/correctness/useExhaustiveDependencies: keyed by the target list, not each re-sorted row array
  React.useEffect(() => {
    if (!enabled || targets.length === 0) return;
    let cancelled = false;
    let wake: (() => void) | null = null;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const sleep = (ms: number) =>
      new Promise<void>((resolve) => {
        wake = resolve;
        timer = setTimeout(resolve, ms);
      });
    const isBusy = () =>
      !isDocumentVisible() ||
      Date.now() - lastInputAtRef.current < RECENT_INPUT_MS ||
      queryClient.isFetching({ queryKey: ["channel-messages"] }) > 0;

    void (async () => {
      for (const target of targets) {
        const key = inboxPreloadTargetKey(target);
        const attemptedAt = attemptedAtRef.current.get(key);
        if (
          attemptedAt !== undefined &&
          Date.now() - attemptedAt < RETRY_AFTER_MS
        ) {
          continue;
        }
        while (!cancelled && isBusy()) await sleep(IDLE_POLL_MS);
        if (cancelled) return;
        attemptedAtRef.current.set(key, Date.now());
        await preloadChannelMessages(
          queryClient,
          target.channelId,
          INBOX_PRELOAD_MESSAGES,
        );
        if (cancelled) return;
        if (target.rootId) {
          await preloadThreadReplies(
            queryClient,
            target.channelId,
            target.rootId,
          );
        }
      }
    })();
    return () => {
      cancelled = true;
      if (timer !== null) clearTimeout(timer);
      wake?.();
    };
  }, [enabled, queryClient, targetsKey]);
}
