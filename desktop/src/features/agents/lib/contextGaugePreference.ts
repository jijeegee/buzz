import * as React from "react";

export const CONTEXT_GAUGE_ENABLED_STORAGE_KEY =
  "buzz.agents.contextGaugeEnabled";
export const DEFAULT_CONTEXT_GAUGE_ENABLED = true;

const listeners = new Set<() => void>();
let contextGaugeEnabled = readStoredPreference();

export function parseContextGaugeEnabled(
  value: string | null | undefined,
): boolean {
  if (value === "false") return false;
  if (value === "true") return true;
  return DEFAULT_CONTEXT_GAUGE_ENABLED;
}

function readStoredPreference(): boolean {
  try {
    return parseContextGaugeEnabled(
      globalThis.localStorage?.getItem(CONTEXT_GAUGE_ENABLED_STORAGE_KEY),
    );
  } catch {
    return DEFAULT_CONTEXT_GAUGE_ENABLED;
  }
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function getContextGaugeEnabled(): boolean {
  return contextGaugeEnabled;
}

/**
 * Show or hide the context gauge and its Compact entry points. Display only:
 * turning it off never touches agent sessions or running turns, and usage
 * readings keep being recorded so the gauge is current when turned back on.
 */
export function setContextGaugeEnabled(value: boolean): void {
  if (value === contextGaugeEnabled) return;
  contextGaugeEnabled = value;
  try {
    globalThis.localStorage?.setItem(
      CONTEXT_GAUGE_ENABLED_STORAGE_KEY,
      String(value),
    );
  } catch {
    // Persistence is best-effort; the live preference still applies.
  }
  for (const listener of listeners) listener();
}

export function useContextGaugeEnabled(): boolean {
  return React.useSyncExternalStore(
    subscribe,
    getContextGaugeEnabled,
    () => DEFAULT_CONTEXT_GAUGE_ENABLED,
  );
}
