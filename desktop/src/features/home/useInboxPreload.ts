import { useQueryClient } from "@tanstack/react-query";
import * as React from "react";

import type { InboxItem } from "@/features/home/lib/inbox";
import {
  INBOX_PRELOAD_BACKGROUND_ROWS,
  INBOX_PRELOAD_FIRST_MESSAGES,
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
import { openWindowsInFlight } from "@/shared/api/openWindowRequests";
import { relayClient } from "@/shared/api/relayClient";
import { isDocumentVisible } from "@/shared/lib/useDocumentVisible";

/** Input this recent means the user is typing or clicking: yield to them. */
const RECENT_INPUT_MS = 1_500;
const IDLE_POLL_MS = 500;
/** A target that failed (or was skipped) is not retried sooner than this. */
const RETRY_AFTER_MS = 5 * 60 * 1_000;
/** A room drawn from the store is looked at again after this. */
const CERTAIN_RETRY_AFTER_MS = 60 * 1_000;
/** The first wave lets a room being opened go first, at most this long. */
const FOREGROUND_FIRST_MAX_MS = 3_000;
const FOREGROUND_POLL_MS = 100;

/** Resolves once the relay socket is connected, or when `signal` aborts. */
function relayConnected(signal: AbortSignal): Promise<void> {
  return new Promise((resolve) => {
    let unsubscribe: (() => void) | null = null;
    let done = false;
    const finish = () => {
      done = true;
      unsubscribe?.();
      signal.removeEventListener("abort", finish);
      resolve();
    };
    if (signal.aborted) return finish();
    signal.addEventListener("abort", finish);
    unsubscribe = relayClient.subscribeToConnectionState((state) => {
      if (state === "connected") finish();
    });
    if (done) unsubscribe();
  });
}

/**
 * Background-loads the rooms behind the top chat-list rows (one batched
 * request), then the newest replies of their threads (four per request), then
 * the rooms and threads of rows up to {@link INBOX_PRELOAD_BACKGROUND_ROWS}
 * (see `runInboxPreload`), so opening them paints at once. Rooms the store
 * holds certainly are not requested (docs/message-sync.md#store-first-preload).
 * The first pass's first wave asks the relay right after the socket connects,
 * {@link INBOX_PRELOAD_FIRST_MESSAGES} rows per room and no store check, once
 * a room being opened has its window (docs/message-sync.md#launch-order); the
 * background wave and later passes pause while the user types or clicks,
 * while a room is loading in the foreground, and while the app is hidden.
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
    // The room on screen asks first: its open window, or its fetch.
    const waitForeground = async () => {
      const until = Date.now() + FOREGROUND_FIRST_MAX_MS;
      while (
        !cancelled &&
        Date.now() < until &&
        (openWindowsInFlight() > 0 ||
          queryClient.isFetching({ queryKey: ["channel-messages"] }) > 0)
      ) {
        await sleep(FOREGROUND_POLL_MS);
      }
      return !cancelled;
    };

    const now = Date.now();
    for (const [key, at] of attemptedAtRef.current) {
      if (now - at >= RETRY_AFTER_MS) attemptedAtRef.current.delete(key);
    }
    const pending = targets.filter(
      (target) => !attemptedAtRef.current.has(inboxPreloadTargetKey(target)),
    );

    // The first pass after launch asks as soon as the relay is connected
    // and the room on screen has its window: that is when the rooms are
    // about to be opened. Later passes yield to the user.
    const firstPass = firstPassRef.current;
    firstPassRef.current = false;
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
          if (!firstPass) return waitIdle();
          await relayConnected(abort.signal);
          return !cancelled && waitForeground();
        },
        waitFirstThreads: async () => (firstPass ? !cancelled : waitIdle()),
        waitIdle,
        preloadRooms: async (channelIds, beforeFetch) => {
          if (cancelled) return;
          for (const channelId of channelIds) {
            attemptedAtRef.current.set(channelId, Date.now());
          }
          // The first wave (no `beforeFetch`) asks at once, a screenful.
          const firstWave = beforeFetch === undefined;
          await preloadChannelsMessages(
            queryClient,
            channelIds,
            firstWave ? INBOX_PRELOAD_FIRST_MESSAGES : INBOX_PRELOAD_MESSAGES,
            {
              signal: abort.signal,
              beforeFetch,
              storeCheck: !firstWave,
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
      for (const [timer, resolve] of sleepers) {
        clearTimeout(timer);
        resolve();
      }
      sleepers.clear();
    };
  }, [enabled, queryClient, targetsKey]);
}
