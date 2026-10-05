import * as React from "react";

import { autoAssignDecision } from "@/features/messages/lib/autoAssignGate";
import {
  type RouteMessageInput,
  type RouteMessageResult,
  type RouterSkipReason,
  routeMessage,
} from "@/shared/api/tauriMessageRouting";
import { normalizePubkey } from "@/shared/lib/pubkey";
import type { RouterRosterSnapshot } from "./useRouterRosterSource";

/**
 * The longest Enter waits for a routing result before sending unassigned,
 * unless the route says otherwise (`sendWaitMs`: subscription CLI routes
 * wait longer).
 */
export const AUTO_ASSIGN_SEND_WAIT_MS = 1_200;
const NOTICE_MS = 3_000;

const SKIP_NOTICE: Record<
  Exclude<RouterSkipReason, "not-configured">,
  string
> = {
  timeout: "routing timed out",
  "provider-error": "routing failed",
  "bad-output": "routing returned no usable answer",
};

export type AutoAssignOptions = {
  channelId: string | null;
  channelType: string | null | undefined;
  isEditing: boolean;
  addressedAgentCount: number;
  routerActive: boolean;
  routerReady: boolean;
  /** Enter's wait budget for the current route; default {@link AUTO_ASSIGN_SEND_WAIT_MS}. */
  sendWaitMs?: number;
  threadRoot: string | null;
  getExplicitMentionCount: (text: string) => number;
  getRoster: () => RouterRosterSnapshot;
  /** Injected in tests; defaults to the `route_message` IPC. */
  route?: (input: RouteMessageInput) => Promise<RouteMessageResult>;
};

/**
 * Smart routing in the composer. Typing never calls the router: on Enter,
 * `resolveForSend` routes the final text exactly once, waiting at most the
 * route's `sendWaitMs`; a slow or failed call sends unassigned with a quiet
 * notice. Sending is never blocked.
 */
export function useAutoAssign(options: AutoAssignOptions) {
  const optionsRef = React.useRef(options);
  optionsRef.current = options;
  const noticeRef = React.useRef<ReturnType<typeof setTimeout> | null>(null);
  const [notice, setNotice] = React.useState<string | null>(null);

  React.useEffect(
    () => () => {
      if (noticeRef.current) clearTimeout(noticeRef.current);
    },
    [],
  );

  const showNotice = React.useCallback((text: string) => {
    if (noticeRef.current) clearTimeout(noticeRef.current);
    setNotice(text);
    noticeRef.current = setTimeout(() => setNotice(null), NOTICE_MS);
  }, []);

  /**
   * The agents to address for this send. Never throws and never waits more
   * than the route's `sendWaitMs`.
   */
  const resolveForSend = React.useCallback(
    async (text: string): Promise<string[]> => {
      const current = optionsRef.current;
      // Off (the common case) costs nothing: no mention scan, no roster.
      if (!current.routerActive) return [];
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
      if (!gate.run) return [];
      const route = current.route ?? routeMessage;
      const routing = route({
        message: text,
        threadRoot: current.threadRoot,
        roster,
        humans,
        phase: "send",
        channelId: current.channelId,
      }).catch(
        (): RouteMessageResult => ({
          decision: "skipped",
          reason: "provider-error",
        }),
      );
      let timer: ReturnType<typeof setTimeout> | null = null;
      const result = await Promise.race([
        routing,
        new Promise<null>((resolve) => {
          timer = setTimeout(
            () => resolve(null),
            current.sendWaitMs ?? AUTO_ASSIGN_SEND_WAIT_MS,
          );
        }),
      ]);
      if (timer) clearTimeout(timer);
      if (result === null) {
        showNotice(`Sent without auto-assign (${SKIP_NOTICE.timeout})`);
        return [];
      }
      if (result.decision === "skipped") {
        if (result.reason !== "not-configured") {
          showNotice(
            `Sent without auto-assign (${SKIP_NOTICE[result.reason]})`,
          );
        }
        return [];
      }
      if (result.decision === "none") return [];
      return result.pubkeys.map(normalizePubkey);
    },
    [showNotice],
  );

  return { notice, resolveForSend };
}
