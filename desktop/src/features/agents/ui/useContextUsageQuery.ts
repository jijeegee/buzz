import * as React from "react";

import {
  awaitContextUsageQuery,
  type ContextQueryState,
} from "@/features/agents/lib/contextUsageQuery";
import {
  ensureRelayObserverSubscription,
  subscribeControlResults,
} from "@/features/agents/observerRelayStore";
import { queryManagedAgentContextUsage } from "@/shared/api/agentControl";

/**
 * Wait this long for the harness answer. An offline harness, or one without
 * `query_context_usage`, never answers; the dialog then falls back to the
 * cached reading marked as possibly stale.
 */
const CONTEXT_QUERY_TIMEOUT_MS = 5_000;

export type ContextUsageQueryTarget = {
  agentPubkey: string;
  channelId: string;
  threadRootEventId: string | null;
};

/**
 * Ask the harness for one session scope's current context once per mount
 * (the Compact dialog mounts on open). A send failure reads as unconfirmed,
 * like a missing answer.
 */
export function useContextUsageQuery(
  target: ContextUsageQueryTarget,
): ContextQueryState {
  const { agentPubkey, channelId, threadRootEventId } = target;
  const [state, setState] = React.useState<ContextQueryState>({
    status: "checking",
  });

  React.useEffect(() => {
    let active = true;
    setState({ status: "checking" });
    const requestId = crypto.randomUUID();
    awaitContextUsageQuery({
      requestId,
      channelId,
      subscribe: (listener) => subscribeControlResults(agentPubkey, listener),
      send: async () => {
        await ensureRelayObserverSubscription();
        await queryManagedAgentContextUsage(
          agentPubkey,
          channelId,
          threadRootEventId,
          requestId,
        );
      },
      scheduleTimeout: (onTimeout) => {
        const timeout = window.setTimeout(onTimeout, CONTEXT_QUERY_TIMEOUT_MS);
        return () => window.clearTimeout(timeout);
      },
    })
      .catch(() => ({ status: "unconfirmed" }) as const)
      .then((result) => {
        if (active) setState(result);
      });
    return () => {
      active = false;
    };
  }, [agentPubkey, channelId, threadRootEventId]);

  return state;
}
