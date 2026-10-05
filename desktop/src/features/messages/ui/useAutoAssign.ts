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
  routeMessage,
} from "@/shared/api/tauriMessageRouting";
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
  getExplicitMentionCount: (text: string) => number;
  getRoster: () => RouterRosterSnapshot;
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
        explicitMentionCount: current.getExplicitMentionCount(text),
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
      return (message) => {
        void routeAndDeliver(message, input, route, deliver);
      };
    },
    [],
  );

  return { routeAfterSend };
}

async function routeAndDeliver(
  message: RelayEvent,
  input: RouteMessageInput,
  route: (input: RouteMessageInput) => Promise<RouteMessageResult>,
  deliver: AutoRouteDeliver,
) {
  const generation = beginAutoRoute(message.id);
  const isCurrent = () => isAutoRouteCurrent(message.id, generation);
  let timer: ReturnType<typeof setTimeout> | null = null;
  try {
    const result = await Promise.race([
      route(input),
      new Promise<null>((resolve) => {
        timer = setTimeout(() => resolve(null), AUTO_ROUTE_RESULT_TIMEOUT_MS);
      }),
    ]);
    if (result === null) {
      settleAutoRoute(message.id, generation, { status: "failed" });
      return;
    }
    if (result.decision === "skipped") {
      settleAutoRoute(
        message.id,
        generation,
        result.reason === "not-configured" ? null : { status: "failed" },
      );
      return;
    }
    if (result.decision === "none" || !isCurrent()) {
      settleAutoRoute(message.id, generation, null);
      return;
    }
    const pubkeys = result.pubkeys.map(normalizePubkey);
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
