import * as React from "react";

import {
  type AgentPairing,
  listAgentPairings,
  type PairingStart,
  startAgentPairing,
} from "@/shared/api/agentPairing";
import { secondsLeft, startPairingPoll } from "./pairingLogic";

export type PairingPhase =
  | { kind: "starting" }
  | { kind: "waiting"; start: PairingStart }
  | { kind: "connected"; start: PairingStart; claim: AgentPairing }
  | { kind: "expired"; start: PairingStart }
  | { kind: "error"; message: string };

function errorMessage(error: unknown, fallback: string) {
  return error instanceof Error
    ? error.message
    : typeof error === "string"
      ? error
      : fallback;
}

/**
 * One connect-screen session: a code when `open` turns on, a check every
 * two seconds while the code waits (stopped when the screen closes or the
 * code expires), and `newCode` to replace it. `onConnected` runs once per
 * connected code.
 */
export function usePairingSession({
  open,
  failedText,
  onConnected,
}: {
  open: boolean;
  /** Shown when making a code fails without its own message. */
  failedText: string;
  onConnected: (claim: AgentPairing) => void;
}): { phase: PairingPhase; secondsLeft: number; newCode: () => void } {
  const [phase, setPhase] = React.useState<PairingPhase>({ kind: "starting" });
  const [now, setNow] = React.useState(() => Date.now());
  // A newer code (or closing) makes older answers stale.
  const generation = React.useRef(0);
  const onConnectedRef = React.useRef(onConnected);
  onConnectedRef.current = onConnected;

  const newCode = React.useCallback(() => {
    const mine = ++generation.current;
    setPhase({ kind: "starting" });
    startAgentPairing()
      .then((start) => {
        if (generation.current !== mine) return;
        setNow(Date.now());
        setPhase({ kind: "waiting", start });
      })
      .catch((error) => {
        if (generation.current !== mine) return;
        setPhase({ kind: "error", message: errorMessage(error, failedText) });
      });
  }, [failedText]);

  React.useEffect(() => {
    if (open) {
      newCode();
      return;
    }
    generation.current += 1;
    setPhase({ kind: "starting" });
  }, [open, newCode]);

  const waiting = phase.kind === "waiting" ? phase.start : null;

  React.useEffect(() => {
    if (!waiting) return;
    const mine = generation.current;
    const expiresAtMs = Date.parse(waiting.expiresAt);
    const tick = window.setInterval(() => setNow(Date.now()), 1_000);
    const stop = startPairingPoll({
      pairingId: waiting.pairingId,
      expiresAtMs: Number.isFinite(expiresAtMs) ? expiresAtMs : 0,
      list: listAgentPairings,
      now: Date.now,
      schedule: (run, ms) => window.setTimeout(run, ms),
      cancel: (handle) => window.clearTimeout(handle as number),
      onConnected: (claim) => {
        if (generation.current !== mine) return;
        setPhase({ kind: "connected", start: waiting, claim });
        onConnectedRef.current(claim);
      },
      onExpired: () => {
        if (generation.current !== mine) return;
        setPhase({ kind: "expired", start: waiting });
      },
    });
    return () => {
      window.clearInterval(tick);
      stop();
    };
  }, [waiting]);

  return {
    phase,
    secondsLeft: waiting ? secondsLeft(waiting.expiresAt, now) : 0,
    newCode,
  };
}
