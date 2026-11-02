import * as React from "react";

import { relayClient } from "@/shared/api/relayClient";
import { shouldTriggerResumeReconnect } from "@/shared/api/relayResumeTriggerPolicy";
import { NetworkChangeFilter } from "@/shared/api/userStreamRules";
import { scheduleAfterForegroundReady } from "@/shared/lib/foregroundReady";

type ScheduleForegroundWork = (callback: () => void) => () => void;

export function installRelayResumeTriggers(
  attempt: () => void,
  schedule: ScheduleForegroundWork = scheduleAfterForegroundReady,
): () => void {
  let cancelPendingResume: (() => void) | null = null;
  const scheduleAttempt = () => {
    if (cancelPendingResume) return;
    cancelPendingResume = schedule(() => {
      cancelPendingResume = null;
      attempt();
    });
  };
  const onVisibilityChange = () => {
    if (document.visibilityState === "visible") scheduleAttempt();
    else {
      cancelPendingResume?.();
      cancelPendingResume = null;
    }
  };

  window.addEventListener("online", scheduleAttempt);
  window.addEventListener("focus", scheduleAttempt);
  document.addEventListener("visibilitychange", onVisibilityChange);

  return () => {
    cancelPendingResume?.();
    window.removeEventListener("online", scheduleAttempt);
    window.removeEventListener("focus", scheduleAttempt);
    document.removeEventListener("visibilitychange", onVisibilityChange);
  };
}

/** The network types the OS reports now (empty: offline). */
export function currentNetworkTypes(nav: Navigator = navigator): string[] {
  if (!nav.onLine) return [];
  const connection = (nav as { connection?: { type?: string } }).connection;
  return [connection?.type ?? "unknown"];
}

/**
 * Calls `onChange` when the set of network types really changes (the user
 * stream's overflow backoff ends then): the state at install is the first
 * report and never a change, and a repeat of the same set is none either.
 */
export function installUserStreamNetworkWatch(
  onChange: () => void,
  types: () => string[] = () => currentNetworkTypes(),
): () => void {
  const filter = new NetworkChangeFilter();
  filter.changed(types());
  const report = () => {
    if (filter.changed(types())) onChange();
  };
  const connection = (
    navigator as { connection?: Partial<EventTarget> }
  ).connection;
  window.addEventListener("online", report);
  window.addEventListener("offline", report);
  connection?.addEventListener?.("change", report);
  return () => {
    window.removeEventListener("online", report);
    window.removeEventListener("offline", report);
    connection?.removeEventListener?.("change", report);
  };
}

/**
 * Event-driven reconnect triggers: network `online`, window `focus`, and
 * visibility→visible coalesce a foreground-ready `resumeReconnect()` attempt
 * when the relay session is degraded (reconnecting/stalled), rate-limited by
 * `RESUME_TRIGGER_MIN_INTERVAL_MS`.
 *
 * Rationale (CMD+R gap audit G1): without these, recovery rides solely on
 * the backoff timer, which WKWebView throttles while the window is occluded
 * or the system was asleep. The moment a user focuses the window to hit
 * CMD+R *is* a focus event — this fires the reconnect first.
 *
 * Uses `resumeReconnect()`, NOT `preconnect()`: resume events bypass the
 * pending backoff timer but must preserve the terminal latch and the AUTH
 * rejection streak. Otherwise a focus/online event arriving during repeated
 * AUTH rejection would reset the consecutive-rejection cap and the session
 * could retry indefinitely instead of surfacing `disconnected`. The
 * terminal state stays user-owned via the reconnect card.
 */
export function useRelayResumeTriggers(): void {
  React.useEffect(() => {
    let lastAttemptAt = -Infinity;

    const attempt = () => {
      const now = Date.now();
      if (
        !shouldTriggerResumeReconnect({
          connectionState: relayClient.getConnectionState(),
          lastAttemptAt,
          now,
        })
      ) {
        return;
      }
      lastAttemptAt = now;
      // resumeReconnect() clears any pending backoff timer and connects now,
      // preserving terminal/AUTH-streak state. Failures re-arm the normal
      // backoff loop; nothing to handle here.
      void relayClient.resumeReconnect().catch(() => {});
    };

    const stopTriggers = installRelayResumeTriggers(attempt);
    const stopNetworkWatch = installUserStreamNetworkWatch(() =>
      relayClient.userStream.networkChange(),
    );
    return () => {
      stopTriggers();
      stopNetworkWatch();
    };
  }, []);
}
