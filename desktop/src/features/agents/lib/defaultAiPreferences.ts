import * as React from "react";

/**
 * Whether channels, forums, and project channels you create start with your
 * default AI as a bot member. This is a desktop-local preference (like the
 * feature overrides in `shared/features/store.ts`), not part of
 * `GlobalAgentConfig`: saving that config restarts every local agent, which
 * a membership preference must never do.
 */
export const DEFAULT_AI_AUTO_JOIN_STORAGE_KEY = "buzz-default-ai-auto-join";
export const DEFAULT_AI_AUTO_JOIN_DEFAULT = true;

const listeners = new Set<() => void>();
let autoJoin = readStoredPreference();

/** Only a stored JSON boolean overrides the default; anything else is ignored. */
export function parseDefaultAiAutoJoin(
  raw: string | null | undefined,
): boolean {
  if (raw === null || raw === undefined) return DEFAULT_AI_AUTO_JOIN_DEFAULT;
  try {
    const parsed: unknown = JSON.parse(raw);
    return typeof parsed === "boolean" ? parsed : DEFAULT_AI_AUTO_JOIN_DEFAULT;
  } catch {
    return DEFAULT_AI_AUTO_JOIN_DEFAULT;
  }
}

function readStoredPreference(): boolean {
  try {
    return parseDefaultAiAutoJoin(
      globalThis.localStorage?.getItem(DEFAULT_AI_AUTO_JOIN_STORAGE_KEY),
    );
  } catch {
    return DEFAULT_AI_AUTO_JOIN_DEFAULT;
  }
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function getDefaultAiAutoJoin(): boolean {
  return autoJoin;
}

export function setDefaultAiAutoJoin(value: boolean): void {
  if (value === autoJoin) return;
  autoJoin = value;
  try {
    globalThis.localStorage?.setItem(
      DEFAULT_AI_AUTO_JOIN_STORAGE_KEY,
      JSON.stringify(value),
    );
  } catch {
    // Persistence is best-effort; the live preference still applies.
  }
  for (const listener of listeners) listener();
}

export function useDefaultAiAutoJoin(): boolean {
  return React.useSyncExternalStore(
    subscribe,
    getDefaultAiAutoJoin,
    () => DEFAULT_AI_AUTO_JOIN_DEFAULT,
  );
}
