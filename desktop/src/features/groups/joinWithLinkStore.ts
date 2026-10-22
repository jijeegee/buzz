import * as React from "react";

/**
 * Open state of the "Join with link" dialog. A module store (not React
 * context) so the ＋ menu, deep links and tests can all open it.
 */
export type JoinWithLinkState = {
  open: boolean;
  /** Link to look up right away (deep link), or "" for an empty box. */
  initialLink: string;
  /** Changes on every open so a repeated link re-runs the lookup. */
  requestId: number;
};

let state: JoinWithLinkState = { open: false, initialLink: "", requestId: 0 };
const listeners = new Set<() => void>();

function setState(next: JoinWithLinkState) {
  state = next;
  for (const listener of listeners) listener();
}

export function openJoinWithLink(initialLink = "") {
  setState({ open: true, initialLink, requestId: state.requestId + 1 });
}

export function closeJoinWithLink() {
  setState({ ...state, open: false });
}

export function useJoinWithLinkState(): JoinWithLinkState {
  return React.useSyncExternalStore(
    (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    () => state,
  );
}
