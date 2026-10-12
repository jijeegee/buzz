import { QueryObserver, useQueryClient } from "@tanstack/react-query";
import * as React from "react";

import type { InboxItem } from "@/features/home/lib/inbox";
import {
  INBOX_PRELOAD_MESSAGES,
  INBOX_PRELOAD_ROOMS_PER_REQUEST,
  type InboxPreloadTarget,
  inboxPreloadTargetKey,
  inboxPreloadTargets,
} from "@/features/home/lib/inboxPreload";
import {
  preloadChannelsMessages,
  preloadThreads,
} from "@/features/messages/channelPreload";
import { useDevTuning } from "@/shared/lib/devTuning";
import {
  channelMessagesKey,
  channelWindowKey,
  threadRepliesKey,
} from "@/features/messages/lib/messageQueryKeys";
import { threadWindowStateKey } from "@/features/messages/lib/threadWindowCache";
import {
  isDocumentVisible,
  subscribeAppFocus,
} from "@/shared/lib/useDocumentVisible";

/** Input this recent means the user is typing or clicking: yield to them. */
const RECENT_INPUT_MS = 1_500;
const IDLE_POLL_MS = 500;
/** A target that failed (or was skipped) is not retried sooner than this. */
const RETRY_AFTER_MS = 5 * 60 * 1_000;

/**
 * Background-loads the rooms behind the top chat-list rows (batched room
 * requests), then the newest replies of their threads (four per request), so
 * opening them paints at once: the rows on screen first, then the rows below.
 * Except for the launch wave it pauses while the user types or clicks, while
 * a room is loading in the foreground, and while the app is hidden.
 */
export function useInboxPreload(items: readonly InboxItem[], enabled: boolean) {
  const queryClient = useQueryClient();
  const lastInputAtRef = React.useRef(0);
  const attemptedAtRef = React.useRef(new Map<string, number>());
  const firstPassRef = React.useRef(true);
  // Coming back to the app re-runs the preload for anything that went missing.
  const [refillTick, setRefillTick] = React.useState(0);
  React.useEffect(() => {
    if (!enabled) return;
    return subscribeAppFocus((focused) => {
      if (!focused) return;
      // Retry rows whose caches are gone, without waiting out the retry delay.
      for (const key of attemptedAtRef.current.keys()) {
        const [channelId, rootId] = key.split(":");
        const missing = rootId
          ? queryClient.getQueryData(threadRepliesKey(channelId, rootId)) ===
            undefined
          : queryClient.getQueryData(channelMessagesKey(channelId)) ===
            undefined;
        if (missing) attemptedAtRef.current.delete(key);
      }
      setRefillTick((tick) => tick + 1);
    });
  }, [enabled, queryClient]);

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

  // Two waves: the rows on screen load at once, the rows below them next.
  const { preloadFirstRows, preloadMoreRows } = useDevTuning();
  const firstTargets = React.useMemo(
    () => inboxPreloadTargets(items, preloadFirstRows),
    [items, preloadFirstRows],
  );
  const moreTargets = React.useMemo(
    () => inboxPreloadTargets(items, preloadMoreRows, preloadFirstRows),
    [items, preloadFirstRows, preloadMoreRows],
  );
  const targetsKey = [
    firstTargets.map(inboxPreloadTargetKey).join("|"),
    moreTargets.map(inboxPreloadTargetKey).join("|"),
  ].join("/");

  // biome-ignore lint/correctness/useExhaustiveDependencies: keyed by the target lists, not each re-sorted row array
  React.useEffect(() => {
    if (!enabled || firstTargets.length + moreTargets.length === 0) return;
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
    const isPending = (target: InboxPreloadTarget) =>
      !attemptedAtRef.current.has(inboxPreloadTargetKey(target));

    // `urgent` skips waiting for the user to be idle (the launch wave).
    const runWave = async (
      wave: readonly InboxPreloadTarget[],
      urgent: boolean,
    ) => {
      const pending = wave.filter(isPending);
      const ready = async () => (urgent || (await waitIdle())) && !cancelled;
      const roomIds = [...new Set(pending.map((target) => target.channelId))];
      for (
        let start = 0;
        start < roomIds.length;
        start += INBOX_PRELOAD_ROOMS_PER_REQUEST
      ) {
        if (!(await ready())) return;
        const chunk = roomIds.slice(
          start,
          start + INBOX_PRELOAD_ROOMS_PER_REQUEST,
        );
        // A thread counts as attempted when its own request starts, so one cut
        // off by a re-sorted list is picked up by the next pass.
        for (const target of pending) {
          if (!target.thread && chunk.includes(target.channelId)) {
            attemptedAtRef.current.set(
              inboxPreloadTargetKey(target),
              Date.now(),
            );
          }
        }
        await preloadChannelsMessages(
          queryClient,
          chunk,
          INBOX_PRELOAD_MESSAGES,
        );
      }
      if (cancelled) return;
      await preloadThreads(
        queryClient,
        pending.flatMap((target) =>
          target.thread
            ? [{ channelId: target.channelId, rootId: target.thread.rootId }]
            : [],
        ),
        async (threads) => {
          if (!(await ready())) return false;
          for (const thread of threads) {
            attemptedAtRef.current.set(
              inboxPreloadTargetKey({
                channelId: thread.channelId,
                thread: { rootId: thread.rootId, replyId: "" },
              }),
              Date.now(),
            );
          }
          return true;
        },
      );
    };

    void (async () => {
      // The first pass after launch starts at once: that is when the rooms
      // are about to be opened. Later passes, and the second wave, yield to
      // the user.
      const firstPass = firstPassRef.current;
      firstPassRef.current = false;
      await runWave(firstTargets, firstPass);
      if (cancelled) return;
      await runWave(moreTargets, false);
    })();
    return () => {
      cancelled = true;
      for (const [timer, resolve] of sleepers) {
        clearTimeout(timer);
        resolve();
      }
      sleepers.clear();
    };
  }, [enabled, queryClient, refillTick, targetsKey]);

  // Rows on the inbox keep their room and thread caches while they are there,
  // so memory cleanup never leaves them colder than at launch. A disabled
  // observer holds a cache without fetching or counting as "active".
  // biome-ignore lint/correctness/useExhaustiveDependencies: keyed by the target lists
  React.useEffect(() => {
    if (!enabled) return;
    const keys = [...firstTargets, ...moreTargets].flatMap((target) => [
      channelMessagesKey(target.channelId),
      channelWindowKey(target.channelId),
      ...(target.thread
        ? [
            threadRepliesKey(target.channelId, target.thread.rootId),
            threadWindowStateKey(target.channelId, target.thread.rootId),
          ]
        : []),
    ]);
    const unsubscribes = keys.map((queryKey) =>
      new QueryObserver(queryClient, { queryKey, enabled: false }).subscribe(
        () => {},
      ),
    );
    return () => {
      for (const unsubscribe of unsubscribes) unsubscribe();
    };
  }, [enabled, queryClient, targetsKey]);
}
