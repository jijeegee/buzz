import * as React from "react";

import { getWorkingAgentPubkeysForChannel } from "@/features/agents/agentWorkingSignal";
import { getSoftMentionPubkeys } from "@/features/messages/lib/agentAddressMention.mjs";
import { autoAssignDecision } from "@/features/messages/lib/autoAssignGate";
import { enqueueAutoRoute } from "@/features/messages/lib/autoRouteBatcher";
import {
  getAutoRouteDelivery,
  getAutoRoutePrior,
  recordAutoRouteDelivery,
  wasAutoRouteDelivered,
} from "@/features/messages/lib/autoRouteLedger";
import {
  beginAutoRoute,
  isAutoRouteCurrent,
  settleAutoRoute,
} from "@/features/messages/lib/autoRouteStatus";
import { getThreadReference } from "@/features/messages/lib/threading";
import {
  type RouteGroup,
  type RouteMessageInput,
  type RouteMessageResult,
  type RouteNote,
  type RouterRecentMessage,
  type RouterRosterEntry,
  routeMessage,
} from "@/shared/api/tauriMessageRouting";
import type { RelayEvent } from "@/shared/api/types";
import { normalizePubkey } from "@/shared/lib/pubkey";
import type { RouterRosterSnapshot } from "./useRouterRosterSource";

/**
 * Upper bound on one routing call before the sent rows show "Not
 * delivered". The router enforces its own (shorter) deadline; this only
 * guards an IPC that never settles, so "Routing…" cannot stick.
 */
export const AUTO_ROUTE_RESULT_TIMEOUT_MS = 30_000;

export type AutoAssignOptions = {
  channelId: string | null;
  channelType: string | null | undefined;
  isEditing: boolean;
  addressedAgentCount: number;
  routerActive: boolean;
  routerReady: boolean;
  threadRoot: string | null;
  getRoster: () => RouterRosterSnapshot;
  /**
   * Cached conversation before `message`, without the batch's own messages
   * (read when the batch fires).
   */
  getRecent?: (
    message: RelayEvent,
    exclude: ReadonlySet<string>,
  ) => RouterRecentMessage[];
  /** Injected in tests; defaults to the `route_message` IPC. */
  route?: (input: RouteMessageInput) => Promise<RouteMessageResult>;
};

/**
 * Delivers a pick to the agents: makes them ready and edits the sent message
 * to address them (with `route`, a follow-up note). Rejects on failure;
 * resolves `false` when `isCurrent()` turned false before the edit was
 * published.
 */
export type AutoRouteDeliver = (
  message: RelayEvent,
  pubkeys: string[],
  isCurrent: () => boolean,
  route?: RouteNote,
) => Promise<boolean>;

/** One sent message waiting in its channel's batch. */
type PendingAutoRoute = {
  message: RelayEvent;
  channelId: string;
  generation: number;
  text: string;
  threadRoot: string | null;
  /** Soft mentions: agent @mentions posted without `p` tags. */
  mentioned: string[];
  roster: RouterRosterEntry[];
  humans: string[];
  getRecent: AutoAssignOptions["getRecent"];
  route: (input: RouteMessageInput) => Promise<RouteMessageResult>;
  deliver: AutoRouteDeliver;
};

/**
 * Smart routing in the composer. Typing never calls the router and sending
 * never waits for it: `routeAfterSend` checks the gate at Enter and returns
 * a callback for the publish, which queues the sent message in its
 * channel's batch (`autoRouteBatcher`). One call routes the batch; each
 * message is delivered at most once. Progress shows on the sent rows
 * (`autoRouteStatus`).
 */
export function useAutoAssign(options: AutoAssignOptions) {
  const optionsRef = React.useRef(options);
  optionsRef.current = options;

  const routeAfterSend = React.useCallback(
    (
      text: string,
      deliver: AutoRouteDeliver,
    ): ((message: RelayEvent) => void) | null => {
      const current = optionsRef.current;
      // Off (the common case) costs nothing: no mention scan, no roster.
      if (!current.routerActive) return null;
      const { roster, humans } = current.getRoster();
      const gate = autoAssignDecision({
        routerActive: current.routerActive,
        routerReady: current.routerReady,
        channelType: current.channelType,
        isEditing: current.isEditing,
        addressedAgentCount: current.addressedAgentCount,
        text,
        rosterSize: roster.length,
      });
      if (!gate.run) return null;
      const { threadRoot, getRecent } = current;
      const route = current.route ?? routeMessage;
      return (message) => {
        const channelId =
          message.tags.find((tag) => tag[0] === "h")?.[1] ?? current.channelId;
        if (!channelId || wasAutoRouteDelivered(message.id)) return;
        const pending: PendingAutoRoute = {
          message,
          channelId,
          generation: beginAutoRoute(message.id),
          text,
          threadRoot,
          mentioned: getSoftMentionPubkeys(message.tags),
          roster,
          humans,
          getRecent,
          route,
          deliver,
        };
        enqueueAutoRoute(channelId, pending, (batch) => {
          void routeBatch(batch);
        });
      };
    },
    [],
  );

  return { routeAfterSend };
}

/**
 * Who gets the message: the router's pick plus any soft-mentioned agent it
 * could not judge (not in its roster). When routing failed, the
 * soft-mentioned agents: the owner named them, so they are the safe
 * fallback. Empty means nobody.
 */
export function autoRouteTargets(
  outcome: { pubkeys: string[] } | "failed",
  mentioned: readonly string[],
  rosterPubkeys: ReadonlySet<string>,
): string[] {
  if (outcome === "failed") return [...mentioned];
  const unjudged = mentioned.filter((pubkey) => !rosterPubkeys.has(pubkey));
  return [...new Set([...outcome.pubkeys, ...unjudged])];
}

/** The `buzz:route` note for a follow-up group, else none. */
export function autoRouteNote(group: RouteGroup): RouteNote | undefined {
  if (group.relation === "new" || !group.of) return undefined;
  return {
    relation: group.relation,
    of: group.of,
    threadRoot: getAutoRouteDelivery(group.of)?.threadRootId ?? group.of,
  };
}

/** Route one batch with a single call, then deliver each message once. */
async function routeBatch(batch: PendingAutoRoute[]) {
  // Edited, deleted, or already delivered since it was queued: skip.
  const live = batch.filter(
    (item) =>
      isAutoRouteCurrent(item.message.id, item.generation) &&
      !wasAutoRouteDelivered(item.message.id),
  );
  const first = live[0];
  const last = live[live.length - 1];
  if (!first || !last) return;
  const batchIds = new Set(live.map((item) => item.message.id));
  const input: RouteMessageInput = {
    messages: live.map((item) => ({
      id: item.message.id,
      text: item.text,
      threadRoot: item.threadRoot,
      mentioned: item.mentioned,
    })),
    roster: last.roster,
    humans: last.humans,
    phase: "send",
    channelId: first.channelId,
    recent: first.getRecent?.(first.message, batchIds) ?? [],
    prior: getAutoRoutePrior(first.channelId),
    working: getWorkingAgentPubkeysForChannel(first.channelId),
  };
  let timer: ReturnType<typeof setTimeout> | null = null;
  let result: RouteMessageResult | null = null;
  try {
    result = await Promise.race([
      last.route(input),
      new Promise<null>((resolve) => {
        timer = setTimeout(() => resolve(null), AUTO_ROUTE_RESULT_TIMEOUT_MS);
      }),
    ]);
  } catch {
    result = null;
  } finally {
    if (timer) clearTimeout(timer);
  }
  const groups = new Map<string, RouteGroup>();
  if (result?.decision === "routed") {
    for (const group of result.groups) {
      for (const id of group.messageIds) groups.set(id, group);
    }
  }
  const notConfigured =
    result?.decision === "skipped" && result.reason === "not-configured";
  const rosterPubkeys = new Set(
    input.roster.map((entry) => normalizePubkey(entry.pubkey)),
  );
  await Promise.all(
    live.map((item) =>
      deliverOne(
        item,
        result?.decision === "routed"
          ? (groups.get(item.message.id) ?? null)
          : "failed",
        notConfigured,
        rosterPubkeys,
      ),
    ),
  );
}

async function deliverOne(
  item: PendingAutoRoute,
  group: RouteGroup | null | "failed",
  notConfigured: boolean,
  rosterPubkeys: ReadonlySet<string>,
) {
  const { message, generation } = item;
  const isCurrent = () => isAutoRouteCurrent(message.id, generation);
  try {
    const pubkeys = autoRouteTargets(
      group === "failed"
        ? "failed"
        : { pubkeys: (group?.pubkeys ?? []).map(normalizePubkey) },
      item.mentioned,
      rosterPubkeys,
    );
    if (!isCurrent()) {
      settleAutoRoute(message.id, generation, null);
      return;
    }
    if (pubkeys.length === 0) {
      // Nobody to deliver to: "Not delivered" only for a real failure.
      settleAutoRoute(
        message.id,
        generation,
        group === "failed" && !notConfigured ? { status: "failed" } : null,
      );
      return;
    }
    const note = group && group !== "failed" ? autoRouteNote(group) : undefined;
    const delivered = await item.deliver(message, pubkeys, isCurrent, note);
    if (delivered) {
      recordAutoRouteDelivery({
        messageId: message.id,
        channelId: item.channelId,
        agents: pubkeys,
        threadRootId: getThreadReference(message.tags).rootId ?? message.id,
        text: item.text,
        at: Date.now(),
      });
    }
    settleAutoRoute(
      message.id,
      generation,
      delivered ? { status: "delivered", pubkeys } : null,
    );
  } catch {
    settleAutoRoute(message.id, generation, { status: "failed" });
  }
}
