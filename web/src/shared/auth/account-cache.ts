import type { QueryClient } from "@tanstack/react-query";
import type { WebSession } from "./web-session";

/** Cancel retired account work and remove every derived query/mutation result.
 * The epoch also remounts the app: component-local repo data must not survive.
 */
export function bindAccountCache(session: WebSession, client: QueryClient) {
  const identity = () => {
    const snapshot = session.getSnapshot();
    return JSON.stringify([
      snapshot.status,
      snapshot.principalId,
      session.getAccountEpoch(),
    ]);
  };
  let previous = identity();
  let epoch = 0;
  const listeners = new Set<() => void>();
  const dispose = session.subscribe(() => {
    const next = identity();
    if (next === previous) return;
    previous = next;
    epoch += 1;
    // Cancellation is synchronous internally; late non-abortable promises
    // cannot repopulate removed Query objects in the new account's cache.
    void client.cancelQueries();
    client.clear();
    for (const listener of listeners) listener();
  });
  return {
    getSnapshot: () => epoch,
    subscribe: (listener: () => void) => {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    dispose,
  };
}
