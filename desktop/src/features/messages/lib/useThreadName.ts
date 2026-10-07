import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { relayClient } from "@/shared/api/relayClient";
import { signRelayEvent } from "@/shared/api/tauri";
import type { RelayEvent } from "@/shared/api/types";
import { KIND_THREAD_NAME } from "@/shared/constants/kinds";
import { isValidThreadName, selectThreadName } from "./threadName";

export function useThreadName(
  channelId: string | null | undefined,
  threadId: string | null | undefined,
) {
  const queryClient = useQueryClient();
  const [subscriptionError, setSubscriptionError] = useState<Error | null>(
    null,
  );
  const [attempt, setAttempt] = useState(0);
  const generation = useRef(0);
  const key = ["thread-name", channelId, threadId];
  const query = useQuery<RelayEvent | null>({
    queryKey: key,
    enabled: Boolean(channelId && threadId),
    queryFn: async () => {
      if (!channelId || !threadId) throw new Error("No thread selected");
      const events = await relayClient.fetchEvents({
        kinds: [KIND_THREAD_NAME],
        "#h": [channelId],
        "#e": [threadId],
        limit: 1,
      });
      return events.reduce(
        (head, event) => selectThreadName(head, event, channelId, threadId),
        queryClient.getQueryData<RelayEvent | null>(key) ?? null,
      );
    },
  });

  // biome-ignore lint/correctness/useExhaustiveDependencies: A manual retry must reopen a failed subscription.
  useEffect(() => {
    if (!channelId || !threadId) return;
    generation.current += 1;
    const controller = new AbortController();
    let active = true;
    let unsubscribe: (() => void) | undefined;
    const queryKey = ["thread-name", channelId, threadId];
    setSubscriptionError(null);
    // History + live are a single subscription: reconnect replays the latest name.
    void relayClient
      .subscribeLive(
        {
          kinds: [KIND_THREAD_NAME],
          "#h": [channelId],
          "#e": [threadId],
          limit: 1,
        },
        (event) => {
          if (active)
            queryClient.setQueryData<RelayEvent | null>(queryKey, (head) =>
              selectThreadName(head ?? null, event, channelId, threadId),
            );
        },
        undefined,
        undefined,
        controller.signal,
      )
      .then((stop) => {
        if (active) unsubscribe = stop;
        else stop();
      })
      .catch((error: unknown) => {
        if (active)
          setSubscriptionError(
            error instanceof Error ? error : new Error(String(error)),
          );
      });
    return () => {
      active = false;
      generation.current += 1;
      controller.abort();
      unsubscribe?.();
    };
  }, [channelId, threadId, queryClient, attempt]);

  return {
    name: query.data?.content ?? "",
    error: query.error ?? subscriptionError,
    retry: () => {
      setAttempt((value) => value + 1);
      void query.refetch();
    },
    save: async (input: string) => {
      if (!channelId || !threadId) throw new Error("No thread selected");
      const saveGeneration = generation.current;
      const isCurrent = () => generation.current === saveGeneration;
      const assertCurrent = () => {
        if (!isCurrent())
          throw new Error("Thread changed before the name was saved");
      };
      const name = input.normalize("NFC").trim();
      if (!isValidThreadName(name))
        throw new Error(
          "Use up to 40 English or 20 Korean characters on one line.",
        );
      const latest = await query.refetch();
      if (latest.error) throw latest.error;
      assertCurrent();
      const event = await signRelayEvent({
        kind: KIND_THREAD_NAME,
        content: name,
        createdAt: Math.max(
          Math.floor(Date.now() / 1000),
          (latest.data?.created_at ?? 0) + 1,
        ),
        tags: [
          ["h", channelId],
          ["e", threadId],
        ],
      });
      assertCurrent();
      await relayClient.publishEvent(
        event,
        "Thread name save timed out",
        "Could not save thread name",
        isCurrent,
      );
      assertCurrent();
      // Token-mode drafts are re-signed by the relay. Only cache the persisted
      // event, never its pre-submit ID (which could win a same-second tie).
      const persisted = await query.refetch();
      assertCurrent();
      if (persisted.error) throw persisted.error;
    },
  };
}
