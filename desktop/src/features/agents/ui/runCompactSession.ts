import * as React from "react";
import { toast } from "sonner";

import {
  awaitCompactSessionOutcome,
  compactSessionOutcomeNotice,
} from "@/features/agents/lib/compactSessionOutcome";
import {
  ensureRelayObserverSubscription,
  subscribeControlResults,
} from "@/features/agents/observerRelayStore";
import { compactManagedAgentSession } from "@/shared/api/agentControl";
import { normalizePubkey } from "@/shared/lib/pubkey";

/** Wait this long for the harness `started` ack. */
const COMPACT_ACK_TIMEOUT_MS = 10_000;
/** Backstop after the ack; the harness reports its own `timeout` sooner. */
const COMPACT_COMPLETION_TIMEOUT_MS = 10 * 60_000;

export type CompactSessionTarget = {
  agentPubkey: string;
  agentName: string;
  channelId: string;
  threadRootEventId: string | null;
};

// Scopes with a compaction in flight, so a second click on any surface cannot
// send a duplicate request for the same session.
const pendingScopes = new Set<string>();
const pendingListeners = new Set<() => void>();

function pendingKey(target: Omit<CompactSessionTarget, "agentName">): string {
  return `${normalizePubkey(target.agentPubkey)}\u0000${target.channelId}\u0000${target.threadRootEventId ?? ""}`;
}

function setPending(key: string, pending: boolean) {
  if (pending) pendingScopes.add(key);
  else pendingScopes.delete(key);
  for (const listener of pendingListeners) listener();
}

function subscribePending(listener: () => void) {
  pendingListeners.add(listener);
  return () => {
    pendingListeners.delete(listener);
  };
}

/** Whether a compaction request is in flight for this session scope. */
export function useCompactSessionPending(
  target: Omit<CompactSessionTarget, "agentName"> | null,
): boolean {
  const key = target ? pendingKey(target) : null;
  const getSnapshot = React.useCallback(
    () => (key ? pendingScopes.has(key) : false),
    [key],
  );
  return React.useSyncExternalStore(subscribePending, getSnapshot);
}

/**
 * Send a `compact_session` control and narrate it through one toast that
 * moves from "requested" to "compacting" to the harness's terminal outcome.
 */
export async function runCompactSession(target: CompactSessionTarget) {
  const key = pendingKey(target);
  if (pendingScopes.has(key)) return;
  setPending(key, true);
  const requestId = crypto.randomUUID();
  const toastId = `compact-session-${requestId}`;
  toast.loading(`Asking ${target.agentName} to compact this session…`, {
    id: toastId,
  });
  try {
    const outcome = await awaitCompactSessionOutcome({
      requestId,
      channelId: target.channelId,
      subscribe: (listener) =>
        subscribeControlResults(target.agentPubkey, listener),
      send: async () => {
        await ensureRelayObserverSubscription();
        await compactManagedAgentSession(
          target.agentPubkey,
          target.channelId,
          target.threadRootEventId,
          requestId,
        );
      },
      scheduleTimeout: (phase, onTimeout) => {
        const timeout = window.setTimeout(
          onTimeout,
          phase === "ack"
            ? COMPACT_ACK_TIMEOUT_MS
            : COMPACT_COMPLETION_TIMEOUT_MS,
        );
        return () => window.clearTimeout(timeout);
      },
      onStarted: () => {
        toast.loading(`Compacting ${target.agentName}'s session context…`, {
          id: toastId,
        });
      },
    });
    const notice = compactSessionOutcomeNotice(outcome, target.agentName);
    toast[notice.tone](notice.message, { id: toastId });
  } catch (error) {
    toast.error(
      error instanceof Error
        ? error.message
        : `Failed to ask ${target.agentName} to compact this session.`,
      { id: toastId },
    );
  } finally {
    setPending(key, false);
  }
}
