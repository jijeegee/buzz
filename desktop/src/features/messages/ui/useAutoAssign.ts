import * as React from "react";

import { autoAssignDecision } from "@/features/messages/lib/autoAssignGate";
import {
  beginAutoRoute,
  isAutoRouteCurrent,
  settleAutoRoute,
} from "@/features/messages/lib/autoRouteStatus";
import {
  type RouteMessageInput,
  type RouteMessageResult,
  type RouterRecentMessage,
  routeMessage,
} from "@/shared/api/tauriMessageRouting";
import { getSoftMentionPubkeys } from "@/features/messages/lib/agentAddressMention.mjs";
import type { RelayEvent } from "@/shared/api/types";
import { normalizePubkey } from "@/shared/lib/pubkey";
import type { RouterRosterSnapshot } from "./useRouterRosterSource";

/**
 * Upper bound on one routing call before the sent row shows "Not delivered".
 * The router enforces its own (shorter) deadline; this only guards an IPC
 * that never settles, so "Routing…" cannot stick.
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
  /** Cached conversation before the sent message (read at publish time). */
  getRecent?: (message: RelayEvent) => RouterRecentMessage[];
  /** Injected in tests; defaults to the `route_message` IPC. */
  route?: (input: RouteMessageInput) => Promise<RouteMessageResult>;
};

/**
 * Delivers a pick to the agents: makes them ready and edits the sent message
 * to address them. Rejects on failure; resolves `false` when `isCurrent()`
 * turned false before the edit was published.
 */
export type AutoRouteDeliver = (
  message: RelayEvent,
  pubkeys: string[],
  isCurrent: () => boolean,
) => Promise<boolean>;

/**
 * Smart routing in the composer. Typing never calls the router and sending
 * never waits for it: `routeAfterSend` checks the gate at Enter and returns
 * a callback for the publish, which then routes the sent text exactly once
 * and delivers the pick. Progress shows on the sent row (`autoRouteStatus`).
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
      const input: RouteMessageInput = {
        message: text,
        threadRoot: current.threadRoot,
        roster,
        humans,
        phase: "send",
        channelId: current.channelId,
      };
      const route = current.route ?? routeMessage;
      const getRecent = current.getRecent;
      return (message) => {
        const recent = getRecent?.(message) ?? [];
        // Soft mentions: the owner's agent @mentions, posted without `p`
        // tags because this call decides who acts on them.
        const mentioned = getSoftMentionPubkeys(message.tags);
        void routeAndDeliver(
          message,
          { ...input, recent, mentioned },
          route,
          deliver,
        );
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
  outcome: { pubkeys: string[] } | "none" | "failed",
  mentioned: readonly string[],
  rosterPubkeys: ReadonlySet<string>,
): string[] {
  if (outcome === "failed") return [...mentioned];
  const unjudged = mentioned.filter((pubkey) => !rosterPubkeys.has(pubkey));
  const picks = outcome === "none" ? [] : outcome.pubkeys;
  return [...new Set([...picks, ...unjudged])];
}

async function routeAndDeliver(
  message: RelayEvent,
  input: RouteMessageInput,
  route: (input: RouteMessageInput) => Promise<RouteMessageResult>,
  deliver: AutoRouteDeliver,
) {
  const generation = beginAutoRoute(message.id);
  const isCurrent = () => isAutoRouteCurrent(message.id, generation);
  const mentioned = input.mentioned ?? [];
  const rosterPubkeys = new Set(
    input.roster.map((entry) => normalizePubkey(entry.pubkey)),
  );
  let timer: ReturnType<typeof setTimeout> | null = null;
  try {
    const result = await Promise.race([
      route(input),
      new Promise<null>((resolve) => {
        timer = setTimeout(() => resolve(null), AUTO_ROUTE_RESULT_TIMEOUT_MS);
      }),
    ]);
    const outcome =
      result === null || result.decision === "skipped"
        ? ("failed" as const)
        : result.decision === "none"
          ? ("none" as const)
          : { pubkeys: result.pubkeys.map(normalizePubkey) };
    const pubkeys = autoRouteTargets(outcome, mentioned, rosterPubkeys);
    if (!isCurrent()) {
      settleAutoRoute(message.id, generation, null);
      return;
    }
    if (pubkeys.length === 0) {
      // Nobody to deliver to: "Not delivered" only for a real failure.
      const notConfigured =
        result?.decision === "skipped" && result.reason === "not-configured";
      settleAutoRoute(
        message.id,
        generation,
        outcome === "failed" && !notConfigured ? { status: "failed" } : null,
      );
      return;
    }
    const delivered = await deliver(message, pubkeys, isCurrent);
    settleAutoRoute(
      message.id,
      generation,
      delivered ? { status: "delivered", pubkeys } : null,
    );
  } catch {
    settleAutoRoute(message.id, generation, { status: "failed" });
  } finally {
    if (timer) clearTimeout(timer);
  }
}
