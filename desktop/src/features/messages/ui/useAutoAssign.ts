import * as React from "react";

import {
  autoAssignDecision,
  routableTextKey,
} from "@/features/messages/lib/autoAssignGate";
import {
  type RouteMessageInput,
  type RouteMessageResult,
  type RouterSkipReason,
  routeMessage,
} from "@/shared/api/tauriMessageRouting";
import { normalizePubkey } from "@/shared/lib/pubkey";
import type { RouterRosterSnapshot } from "./useRouterRosterSource";

/** Typing pause before a preview call. */
export const AUTO_ASSIGN_DEBOUNCE_MS = 900;
/**
 * The longest Enter waits for a routing result before sending unassigned,
 * unless the route says otherwise (`sendWaitMs`: subscription CLI routes
 * wait longer).
 */
export const AUTO_ASSIGN_SEND_WAIT_MS = 1_200;
/** Preview calls per draft; past it only the Enter-time call runs. */
export const AUTO_ASSIGN_MAX_PREVIEW_CALLS = 6;
/** Distinct draft texts whose results are remembered per draft. */
const RESULT_CACHE_LIMIT = 8;
const NOTICE_MS = 3_000;

const SKIP_NOTICE: Record<
  Exclude<RouterSkipReason, "not-configured">,
  string
> = {
  timeout: "routing timed out",
  "provider-error": "routing failed",
  "bad-output": "routing returned no usable answer",
};

type CacheEntry = {
  promise: Promise<RouteMessageResult>;
  result: RouteMessageResult | null;
};

export type AutoAssignOptions = {
  channelId: string | null;
  channelType: string | null | undefined;
  /** Changing it starts a fresh draft (cache and call budget). */
  draftKey: string | null | undefined;
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
 * Smart routing in the composer. While the user types, a debounced preview
 * call routes the draft in the background and caches the pick; nothing is
 * shown before Enter. On Enter, `resolveForSend` returns the pick for the
 * final text — from the cache, the in-flight call, or a fresh one — waiting
 * at most the route's `sendWaitMs`; a slow or failed call sends
 * unassigned with a quiet notice. Sending is never blocked.
 *
 * Results are keyed by text, so a late answer for an older draft text is
 * never used for the current one. State is React-only (no module
 * singletons), reset when the draft key changes or the draft empties.
 */
export function useAutoAssign(options: AutoAssignOptions) {
  const optionsRef = React.useRef(options);
  optionsRef.current = options;
  const draftRef = React.useRef({
    cache: new Map<string, CacheEntry>(),
    calls: 0,
    latestKey: "",
  });
  const debounceRef = React.useRef<ReturnType<typeof setTimeout> | null>(null);
  const noticeRef = React.useRef<ReturnType<typeof setTimeout> | null>(null);
  const [notice, setNotice] = React.useState<string | null>(null);

  const clearDebounce = React.useCallback(() => {
    if (debounceRef.current) clearTimeout(debounceRef.current);
    debounceRef.current = null;
  }, []);

  const resetDraft = React.useCallback(() => {
    clearDebounce();
    draftRef.current = { cache: new Map(), calls: 0, latestKey: "" };
  }, [clearDebounce]);

  // biome-ignore lint/correctness/useExhaustiveDependencies: draftKey is the sole trigger
  React.useEffect(() => resetDraft(), [options.draftKey]);
  React.useEffect(
    () => () => {
      clearDebounce();
      if (noticeRef.current) clearTimeout(noticeRef.current);
    },
    [clearDebounce],
  );

  const showNotice = React.useCallback((text: string) => {
    if (noticeRef.current) clearTimeout(noticeRef.current);
    setNotice(text);
    noticeRef.current = setTimeout(() => setNotice(null), NOTICE_MS);
  }, []);

  /** Gate for `text`; the roster is only built when every cheap check passes. */
  const decide = React.useCallback((text: string, roster: number | null) => {
    const current = optionsRef.current;
    // Off (the common case) costs nothing: no mention scan, no roster.
    if (!current.routerActive) {
      return { run: false, reason: "router-off" } as const;
    }
    return autoAssignDecision({
      routerActive: current.routerActive,
      routerReady: current.routerReady,
      channelType: current.channelType,
      isEditing: current.isEditing,
      addressedAgentCount: current.addressedAgentCount,
      explicitMentionCount: current.getExplicitMentionCount(text),
      text,
      // Typing-time checks skip the roster; the call site re-checks it.
      rosterSize: roster ?? 1,
    });
  }, []);

  const start = React.useCallback(
    (key: string, text: string, phase: RouteMessageInput["phase"]) => {
      const current = optionsRef.current;
      const { roster, humans } = current.getRoster();
      if (roster.length === 0) return null;
      const route = current.route ?? routeMessage;
      const entry: CacheEntry = {
        result: null,
        promise: Promise.resolve({ decision: "none" }),
      };
      entry.promise = route({
        message: text,
        threadRoot: current.threadRoot,
        roster,
        humans,
        phase,
        channelId: current.channelId,
      })
        .catch(
          (): RouteMessageResult => ({
            decision: "skipped",
            reason: "provider-error",
          }),
        )
        .then((result) => {
          entry.result = result;
          return result;
        });
      const { cache } = draftRef.current;
      cache.set(key, entry);
      while (cache.size > RESULT_CACHE_LIMIT) {
        const oldest = cache.keys().next().value;
        if (oldest === undefined) break;
        cache.delete(oldest);
      }
      return entry;
    },
    [],
  );

  const runPreview = React.useCallback(
    (key: string, text: string) => {
      const draft = draftRef.current;
      if (draft.latestKey !== key || draft.cache.has(key)) return;
      if (draft.calls >= AUTO_ASSIGN_MAX_PREVIEW_CALLS) return;
      if (start(key, text, "preview")) draft.calls += 1;
    },
    [start],
  );

  /** Feed every composer text change; cheap when Smart routing is off. */
  const onText = React.useCallback(
    (text: string) => {
      if (!optionsRef.current.routerActive) return;
      const key = routableTextKey(text);
      if (key === "") {
        if (draftRef.current.latestKey !== "" || draftRef.current.calls > 0) {
          resetDraft();
        }
        return;
      }
      const draft = draftRef.current;
      if (key === draft.latestKey) return;
      draft.latestKey = key;
      clearDebounce();
      if (!decide(text, null).run || draft.cache.has(key)) return;
      debounceRef.current = setTimeout(
        () => runPreview(key, text),
        AUTO_ASSIGN_DEBOUNCE_MS,
      );
    },
    [clearDebounce, decide, resetDraft, runPreview],
  );

  /**
   * The agents to address for this send. Never throws and never waits more
   * than the route's `sendWaitMs`.
   */
  const resolveForSend = React.useCallback(
    async (text: string): Promise<string[]> => {
      clearDebounce();
      const key = routableTextKey(text);
      const draft = draftRef.current;
      draft.latestKey = key;
      if (!decide(text, null).run) return [];
      const rosterSize = optionsRef.current.getRoster().roster.length;
      if (!decide(text, rosterSize).run) return [];
      const entry = draft.cache.get(key) ?? start(key, text, "send");
      if (!entry) return [];
      let timer: ReturnType<typeof setTimeout> | null = null;
      const result =
        entry.result ??
        (await Promise.race([
          entry.promise,
          new Promise<null>((resolve) => {
            timer = setTimeout(
              () => resolve(null),
              optionsRef.current.sendWaitMs ?? AUTO_ASSIGN_SEND_WAIT_MS,
            );
          }),
        ]));
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
    [clearDebounce, decide, showNotice, start],
  );

  return { notice, onText, resolveForSend };
}
