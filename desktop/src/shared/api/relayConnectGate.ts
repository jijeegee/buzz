/**
 * Holds relay socket connects until the active account's saved sign-in has
 * been restored to a settled state (signed in, ended, or no session). Launch
 * draws the stored account before that restore
 * (docs/message-sync.md#launch-order); the socket authenticates only after
 * it, so it never answers AUTH from a half-restored session.
 */

/** A hold this long shows the connection card, so the app does not look live. */
export const SLOW_HOLD_MS = 3_000;

let gate: Promise<void> = Promise.resolve();
let releaseCurrent: (() => void) | null = null;
let slow = false;
const listeners = new Set<() => void>();

function setSlow(next: boolean) {
  if (slow === next) return;
  slow = next;
  for (const listener of listeners) listener();
}

/**
 * Hold connects until the returned function is called. A new hold, or
 * {@link releaseRelayConnect}, releases the previous one.
 */
export function holdRelayConnect(): () => void {
  releaseRelayConnect();
  let resolveGate = () => {};
  gate = new Promise<void>((resolve) => {
    resolveGate = resolve;
  });
  const timer = setTimeout(() => setSlow(true), SLOW_HOLD_MS);
  const release = () => {
    if (releaseCurrent !== release) return;
    releaseCurrent = null;
    clearTimeout(timer);
    setSlow(false);
    resolveGate();
  };
  releaseCurrent = release;
  return release;
}

/** Release any hold, as a community switch does before its own restore. */
export function releaseRelayConnect(): void {
  releaseCurrent?.();
}

/** Resolves once relay connects may start. */
export function relayConnectAllowed(): Promise<void> {
  return gate;
}

/** Connects have been held longer than {@link SLOW_HOLD_MS}. */
export function relayConnectHeldSlow(): boolean {
  return slow;
}

export function subscribeRelayConnectHold(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}
