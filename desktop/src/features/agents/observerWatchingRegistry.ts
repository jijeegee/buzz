/**
 * Agent-keyed registry for the observer `watching` signal.
 *
 * While a live activity view of an agent is visible, the desktop tells that
 * agent's executor "someone is watching" with a `watching` control frame:
 * once immediately, then every {@link WATCHING_INTERVAL_MS}. The executor
 * sends tier-level telemetry for 60 s after the last frame and free-level
 * summaries otherwise, so one missed frame does not drop detail.
 *
 * Several panels can show the same agent at once (channel thread panel,
 * profile activity, ...). Each acquires a hold; the registry keeps ONE timer
 * and sends ONE frame per interval per agent regardless of the hold count.
 * Frames are sent only while the relay is connected; a reconnect sends one
 * immediately for every held agent and restarts its timer.
 *
 * A failed send is not retried on its own: the next interval tick (or the
 * next reconnect) is the retry, and the executor window outlasts one miss.
 */

/** Interval between `watching` frames for one agent. */
export const WATCHING_INTERVAL_MS = 30_000;

export type ObserverWatchingDeps = {
  /** Publish one `watching` frame for `agentPubkey`. */
  send: (agentPubkey: string, channelId: string | null) => Promise<void>;
  /** Current relay connectivity. */
  isConnected: () => boolean;
  /** Notify on connectivity changes; returns an unsubscribe function. */
  subscribeConnection: (listener: (connected: boolean) => void) => () => void;
  setInterval?: (callback: () => void, ms: number) => unknown;
  clearInterval?: (handle: unknown) => void;
  onSendError?: (error: unknown) => void;
};

type AgentWatch = {
  /** Channel per hold, in acquire order; the newest names the frame. */
  holds: Map<symbol, string | null>;
  timer: unknown | null;
};

export type ObserverWatchingRegistry = {
  /** Hold `agentPubkey` as watched; returns the release function. */
  acquire: (agentPubkey: string, channelId: string | null) => () => void;
  /** Agents currently held (for tests and diagnostics). */
  heldAgents: () => string[];
};

export function createObserverWatchingRegistry(
  deps: ObserverWatchingDeps,
): ObserverWatchingRegistry {
  const startTimer =
    deps.setInterval ??
    ((callback: () => void, ms: number) =>
      globalThis.setInterval(callback, ms));
  const stopTimer =
    deps.clearInterval ??
    ((handle: unknown) =>
      globalThis.clearInterval(handle as ReturnType<typeof setInterval>));
  const agents = new Map<string, AgentWatch>();
  let connected = deps.isConnected();
  let unsubscribeConnection: (() => void) | null = null;

  const channelOf = (watch: AgentWatch) => {
    let channelId: string | null = null;
    for (const held of watch.holds.values()) channelId = held;
    return channelId;
  };

  const sendNow = (agentPubkey: string, watch: AgentWatch) => {
    deps.send(agentPubkey, channelOf(watch)).catch((error) => {
      deps.onSendError?.(error);
    });
  };

  const stop = (watch: AgentWatch) => {
    if (watch.timer !== null) {
      stopTimer(watch.timer);
      watch.timer = null;
    }
  };

  /** Send now and (re)start the interval. */
  const start = (agentPubkey: string, watch: AgentWatch) => {
    stop(watch);
    sendNow(agentPubkey, watch);
    watch.timer = startTimer(() => {
      if (connected) sendNow(agentPubkey, watch);
    }, WATCHING_INTERVAL_MS);
  };

  const onConnection = (next: boolean) => {
    if (next === connected) return;
    connected = next;
    for (const [agentPubkey, watch] of agents) {
      if (connected) start(agentPubkey, watch);
      else stop(watch);
    }
  };

  const ensureConnectionListener = () => {
    if (unsubscribeConnection) return;
    unsubscribeConnection = deps.subscribeConnection(onConnection);
    // The subscription may have reported a state that differs from the
    // snapshot taken at construction.
    onConnection(deps.isConnected());
  };

  const dropConnectionListenerIfIdle = () => {
    if (agents.size > 0 || !unsubscribeConnection) return;
    unsubscribeConnection();
    unsubscribeConnection = null;
  };

  return {
    acquire(agentPubkey, channelId) {
      const key = agentPubkey.toLowerCase();
      const token = Symbol(key);
      ensureConnectionListener();
      let watch = agents.get(key);
      if (!watch) {
        watch = { holds: new Map(), timer: null };
        agents.set(key, watch);
      }
      watch.holds.set(token, channelId);
      if (watch.holds.size === 1 && connected) {
        start(key, watch);
      }
      let released = false;
      return () => {
        if (released) return;
        released = true;
        const current = agents.get(key);
        if (!current) return;
        current.holds.delete(token);
        if (current.holds.size === 0) {
          stop(current);
          agents.delete(key);
          dropConnectionListenerIfIdle();
        }
      };
    },
    heldAgents: () => [...agents.keys()],
  };
}
