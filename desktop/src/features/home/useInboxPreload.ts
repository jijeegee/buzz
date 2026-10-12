import { useQueryClient } from "@tanstack/react-query";
import * as React from "react";

import type { InboxItem } from "@/features/home/lib/inbox";
import {
  INBOX_PRELOAD_MESSAGES,
  inboxPreloadTargetKey,
  inboxPreloadTargets,
} from "@/features/home/lib/inboxPreload";
import {
  preloadChannelsMessages,
  preloadThreadReplies,
} from "@/features/messages/channelPreload";
import { threadRepliesKey } from "@/features/messages/lib/messageQueryKeys";
import { isDocumentVisible } from "@/shared/lib/useDocumentVisible";

/** Input this recent means the user is typing or clicking: yield to them. */
const RECENT_INPUT_MS = 1_500;
const IDLE_POLL_MS = 500;
/** Threads loading at once; rooms all load in one batched request. */
const THREAD_PRELOAD_CONCURRENCY = 4;
/** A target that failed (or was skipped) is not retried sooner than this. */
const RETRY_AFTER_MS = 5 * 60 * 1_000;

/**
 * Background-loads the rooms behind the top chat-list rows (one batched
 * request), then their threads a few at a time, so opening them paints at once. Pauses while the user types or clicks,
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
    const sleepers = new Map<ReturnType<typeof setTimeout>, () => void>();
    const sleep = (ms: number) =>
      new Promise<void>((resolve) => {
        const timer = setTimeout(() => {
          sleepers.delete(timer);
          resolve();
        }, ms);
        sleepers.set(timer, resolve);
      });
    // Our own thread loads also count as fetching; only foreground loads yield.
    let ownReplies = 0;
    const isBusy = () =>
      !isDocumentVisible() ||
      Date.now() - lastInputAtRef.current < RECENT_INPUT_MS ||
      queryClient.isFetching({ queryKey: ["channel-messages"] }) > 0 ||
      queryClient.isFetching({ queryKey: ["thread-replies"] }) > ownReplies;
    const waitIdle = async () => {
      while (!cancelled && isBusy()) await sleep(IDLE_POLL_MS);
      return !cancelled;
    };

    const now = Date.now();
    for (const [key, at] of attemptedAtRef.current) {
      if (now - at >= RETRY_AFTER_MS) attemptedAtRef.current.delete(key);
    }
    const pending = targets.filter(
      (target) => !attemptedAtRef.current.has(inboxPreloadTargetKey(target)),
    );
    const threads = pending.filter((target) => target.thread !== null);
    const threadWorker = async () => {
      for (let target = threads.shift(); target; target = threads.shift()) {
        const thread = target.thread;
        if (!thread || !(await waitIdle())) return;
        attemptedAtRef.current.set(inboxPreloadTargetKey(target), Date.now());
        // Count only our own replies fetch, never a cache hit.
        const own =
          queryClient.getQueryData(
            threadRepliesKey(target.channelId, thread.rootId),
          ) === undefined
            ? 1
            : 0;
        ownReplies += own;
        try {
          await preloadThreadReplies(
            queryClient,
            target.channelId,
            thread.rootId,
            thread.replyId,
          );
        } finally {
          ownReplies -= own;
        }
      }
    };

    void (async () => {
      if (pending.length === 0 || !(await waitIdle())) return;
      // A thread counts as attempted when its own load starts, so one cut off
      // by a re-sorted list is picked up by the next pass.
      for (const target of pending) {
        if (target.thread) continue;
        attemptedAtRef.current.set(inboxPreloadTargetKey(target), Date.now());
      }
      await preloadChannelsMessages(
        queryClient,
        [...new Set(pending.map((target) => target.channelId))],
        INBOX_PRELOAD_MESSAGES,
      );
      if (cancelled) return;
      await Promise.all(
        Array.from({ length: THREAD_PRELOAD_CONCURRENCY }, threadWorker),
      );
    })();
    return () => {
      cancelled = true;
      for (const [timer, resolve] of sleepers) {
        clearTimeout(timer);
        resolve();
      }
      sleepers.clear();
    };
  }, [enabled, queryClient, targetsKey]);
}
