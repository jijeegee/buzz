import * as React from "react";

type CommitCounterWindow = Window & {
  __KUZZ_E2E__?: unknown;
  __KUZZ_E2E_COMMIT_COUNTS__?: Record<string, number>;
};

function useCommitCounter(name: string) {
  // No deps: runs after every commit in which the calling component rendered.
  React.useEffect(() => {
    const w = window as CommitCounterWindow;
    if (!w.__KUZZ_E2E__) return;
    const counts = w.__KUZZ_E2E_COMMIT_COUNTS__ ?? {};
    counts[name] = (counts[name] ?? 0) + 1;
    w.__KUZZ_E2E_COMMIT_COUNTS__ = counts;
  });
}

function noop(_name: string) {}

/**
 * E2E-only render counter: e2e specs read `window.__KUZZ_E2E_COMMIT_COUNTS__`
 * to assert a component does not re-render. React.Profiler is inert in the
 * production React build the e2e suite runs, so this counts commits directly.
 * Release builds get a no-op.
 */
export const useE2eCommitCounter =
  import.meta.env.DEV || import.meta.env.MODE === "e2e"
    ? useCommitCounter
    : noop;
