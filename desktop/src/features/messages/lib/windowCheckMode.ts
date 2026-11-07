/**
 * Which way windows open (docs/message-sync.md#window-check): `off` is
 * today's path (always fetch the page), `shadow` checks and logs but still
 * fetches, `on` skips the page when the check matches. The relay sends its
 * mode in every check answer; a developer setting can override it.
 */
import * as React from "react";

export type WindowCheckMode = "off" | "shadow" | "on";
export type WindowCheckOverride = "follow" | WindowCheckMode;

export const WINDOW_CHECK_OVERRIDE_STORAGE_KEY =
  "kuzz.messages.windowCheckOverride";
export const WINDOW_CHECK_OVERRIDES: readonly WindowCheckOverride[] = [
  "follow",
  "off",
  "shadow",
  "on",
];
/** After the relay said `off`, ask it again after this long. */
export const WINDOW_CHECK_REASK_MS = 10 * 60_000;

export function parseWindowCheckOverride(
  value: string | null | undefined,
): WindowCheckOverride {
  return WINDOW_CHECK_OVERRIDES.find((mode) => mode === value) ?? "follow";
}

function readStoredOverride(): WindowCheckOverride {
  try {
    return parseWindowCheckOverride(
      globalThis.localStorage?.getItem(WINDOW_CHECK_OVERRIDE_STORAGE_KEY),
    );
  } catch {
    return "follow";
  }
}

let override = readStoredOverride();
const listeners = new Set<() => void>();

export function getWindowCheckOverride(): WindowCheckOverride {
  return override;
}

export function setWindowCheckOverride(value: WindowCheckOverride) {
  if (value === override) return;
  override = value;
  try {
    globalThis.localStorage?.setItem(WINDOW_CHECK_OVERRIDE_STORAGE_KEY, value);
  } catch {
    // Persistence is best-effort; the live setting still applies.
  }
  for (const listener of listeners) listener();
}

export function useWindowCheckOverride(): WindowCheckOverride {
  return React.useSyncExternalStore(
    (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    getWindowCheckOverride,
    () => "follow" as const,
  );
}

/** Each relay's mode from its latest check answer, and when it came. */
const serverModes = new Map<string, { mode: WindowCheckMode; at: number }>();

/** A check answer from `relay` named `mode` (`off` for a relay without it). */
export function noteServerWindowCheckMode(
  relay: string,
  mode: WindowCheckMode,
  atMs: number,
) {
  serverModes.set(relay, { mode, at: atMs });
}

/**
 * The mode for a window on `relay` now: the override, else the relay's
 * latest; `probe` when that is not known (no check yet this run) or was
 * `off` long enough ago to ask again: send a check to learn it.
 */
export function windowCheckModeFor(
  relay: string,
  nowMs: number,
): WindowCheckMode | "probe" {
  if (override !== "follow") return override;
  const known = serverModes.get(relay);
  if (!known) return "probe";
  if (known.mode === "off" && nowMs - known.at >= WINDOW_CHECK_REASK_MS) {
    return "probe";
  }
  return known.mode;
}

/** The mode a check answer on `relay` leads to: the override, else its own. */
export function effectiveWindowCheckMode(
  answered: WindowCheckMode,
): WindowCheckMode {
  return override === "follow" ? answered : override;
}

/**
 * Whether any window may open by a check now: the override says `on` or
 * `shadow`, or (following the relay) one has answered so. With none, the
 * store keeps today's facts untouched (mode off is today).
 */
export function windowChecksActive(): boolean {
  if (override !== "follow") return override !== "off";
  for (const { mode } of serverModes.values()) if (mode !== "off") return true;
  return false;
}

/** Forget the relays' modes and the override (tests). */
export function resetWindowCheckModes(next: WindowCheckOverride = "follow") {
  serverModes.clear();
  override = next;
}
