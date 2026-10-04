import type { QueryClient } from "@tanstack/react-query";
import type { WebSession } from "./web-session";

/** Cancel retired account work and remove every derived query/mutation result.
 * The epoch also remounts the app: component-local repo data must not survive.
 *
 * Only an account change counts: a different principal, or signing out. A
 * status blip (signed_in → unavailable → signed_in) is the same account, and
 * so is learning the principal id after a restore (`unidentified` → that
 * principal within one session generation): its data was fetched with that
 * principal's token.
 */
export function bindAccountCache(session: WebSession, client: QueryClient) {
  const identity = () => {
    const snapshot = session.getSnapshot();
    if (snapshot.status === "signed_out") return "guest";
    if (snapshot.principalId) return `principal:${snapshot.principalId}`;
    return `unidentified:${session.getAccountEpoch()}`;
  };
  let previous = identity();
  let previousEpoch = session.getAccountEpoch();
  let epoch = 0;
  const listeners = new Set<() => void>();
  const dispose = session.subscribe(() => {
    const next = identity();
    const accountEpoch = session.getAccountEpoch();
    const learnedPrincipal =
      previous.startsWith("unidentified:") &&
      next.startsWith("principal:") &&
      accountEpoch === previousEpoch;
    if (next === previous || learnedPrincipal) {
      previous = next;
      previousEpoch = accountEpoch;
      return;
    }
    previous = next;
    previousEpoch = accountEpoch;
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
