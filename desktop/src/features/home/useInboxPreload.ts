import { useQueryClient } from "@tanstack/react-query";
import * as React from "react";

import type { InboxItem } from "@/features/home/lib/inbox";
import {
  INBOX_PRELOAD_BACKGROUND_ROWS,
  INBOX_PRELOAD_MESSAGES,
  inboxPreloadTargetKey,
  inboxPreloadTargets,
} from "@/features/home/lib/inboxPreload";
import { runInboxPreload } from "@/features/home/lib/inboxPreloadRun";
import {
  type PreloadThread,
  preloadChannelsMessages,
  preloadThreads,
} from "@/features/messages/channelPreload";
import { isDocumentVisible } from "@/shared/lib/useDocumentVisible";

/** Input this recent means the user is typing or clicking: yield to them. */
const RECENT_INPUT_MS = 1_500;
const IDLE_POLL_MS = 500;
/** A target that failed (or was skipped) is not retried sooner than this. */
const RETRY_AFTER_MS = 5 * 60 * 1_000;
/** A room drawn from the store is looked at again after this. */
const CERTAIN_RETRY_AFTER_MS = 60 * 1_000;
/** The first pass waits for the browser to be idle, at most this long. */
const STARTUP_IDLE_TIMEOUT_MS = 2_000;

/** Resolves when the browser is idle (or after `timeoutMs`); `cancel` ends it. */
function idleCallback(timeoutMs: number) {
  let cancel = () => {};
  const done = new Promise<void>((resolve) => {
    if ("requestIdleCallback" in window) {
      const id = window.requestIdleCallback(() => resolve(), {
        timeout: timeoutMs,
      });
      cancel = () => {
        window.cancelIdleCallback(id);
        resolve();
      };
    } else {
      const timer = setTimeout(resolve, timeoutMs);
      cancel = () => {
        clearTimeout(timer);
        resolve();
      };
    }
  });
  return { done, cancel };
}

/**
 * Background-loads the rooms behind the top chat-list rows (one batched
 * request), then the newest replies of their threads (four per request), then
 * the rooms and threads of rows up to {@link INBOX_PRELOAD_BACKGROUND_ROWS}
 * (see `runInboxPreload`), so opening them paints at once. Rooms the store
 * holds certainly are not requested (docs/message-sync.md#store-first-preload).
 * The first pass starts once the browser is idle after launch, never on the
 * startup path; the background wave and later passes pause while the user
 * types or clicks, while a room is loading in the foreground, and while the
 * app is hidden.
 */
export function useInboxPreload(items: readonly InboxItem[], enabled: boolean) {
  const queryClient = useQueryClient();
  const lastInputAtRef = React.useRef(0);
  const attemptedAtRef = React.useRef(new Map<string, number>());
  const firstPassRef = React.useRef(true);

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

  const targets = React.useMemo(
    () => inboxPreloadTargets(items, INBOX_PRELOAD_BACKGROUND_ROWS),
    [items],
  );
  const targetsKey = targets.map(inboxPreloadTargetKey).join("|");

  // biome-ignore lint/correctness/useExhaustiveDependencies: keyed by the target list, not each re-sorted row array
  React.useEffect(() => {
    if (!enabled || targets.length === 0) return;
    let cancelled = false;
    // Ends a store wait or relay request of this pass on sign-out, account
    // switch or leaving Home.
    const abort = new AbortController();
    const sleepers = new Map<ReturnType<typeof setTimeout>, () => void>();
    const sleep = (ms: number) =>
      new Promise<void>((resolve) => {
        const timer = setTimeout(() => {
          sleepers.delete(timer);
          resolve();
        }, ms);
        sleepers.set(timer, resolve);
      });
    const isBusy = () =>
      !isDocumentVisible() ||
      Date.now() - lastInputAtRef.current < RECENT_INPUT_MS ||
      queryClient.isFetching({ queryKey: ["channel-messages"] }) > 0 ||
      queryClient.isFetching({ queryKey: ["thread-replies"] }) > 0;
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

    // The first pass after launch waits only for the browser to be idle:
    // that is when the rooms are about to be opened. Later passes yield to
    // the user.
    const firstPass = firstPassRef.current;
    firstPassRef.current = false;
    const idle = firstPass ? idleCallback(STARTUP_IDLE_TIMEOUT_MS) : null;
    const markThreads = (threads: readonly PreloadThread[]) => {
      for (const thread of threads) {
        attemptedAtRef.current.set(
          inboxPreloadTargetKey({
            channelId: thread.channelId,
            thread: { rootId: thread.rootId, replyId: "" },
          }),
          Date.now(),
        );
      }
    };
    if (pending.length > 0) {
      void runInboxPreload(pending, {
        waitStart: async () => {
          if (!idle) return waitIdle();
          await idle.done;
          return !cancelled;
        },
        waitFirstThreads: async () => (firstPass ? !cancelled : waitIdle()),
        waitIdle,
        preloadRooms: async (channelIds, beforeFetch) => {
          if (cancelled) return;
          for (const channelId of channelIds) {
            attemptedAtRef.current.set(channelId, Date.now());
          }
          await preloadChannelsMessages(
            queryClient,
            channelIds,
            INBOX_PRELOAD_MESSAGES,
            {
              signal: abort.signal,
              beforeFetch,
              // Store-drawn rooms are looked at again sooner than failures.
              onCertain: (roomIds) => {
                const at =
                  Date.now() - (RETRY_AFTER_MS - CERTAIN_RETRY_AFTER_MS);
                for (const roomId of roomIds) {
                  attemptedAtRef.current.set(roomId, at);
                }
              },
            },
          );
        },
        // A thread counts as attempted when its own request starts, so one
        // cut off by a re-sorted list is picked up by the next pass.
        preloadThreads: (threads, beforeRequest) =>
          preloadThreads(queryClient, threads, async (chunk) => {
            if (!(await beforeRequest(chunk)) || cancelled) return false;
            markThreads(chunk);
            return true;
          }),
      });
    }
    return () => {
      cancelled = true;
      abort.abort();
      idle?.cancel();
      for (const [timer, resolve] of sleepers) {
        clearTimeout(timer);
        resolve();
      }
      sleepers.clear();
    };
  }, [enabled, queryClient, targetsKey]);
}
