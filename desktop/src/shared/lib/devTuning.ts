import * as React from "react";

import { invokeTauri } from "@/shared/api/tauri";

/**
 * Hand-edited knobs read once at launch from `dev-tuning.json` in the app data
 * folder, for trying performance trade-offs on an installed build. Not a user
 * setting: there is no UI, and a missing or malformed file means defaults.
 */
export type DevTuning = {
  /** Recently opened rooms kept rendered (hidden) for instant switching. */
  warmRooms: number;
  /** Top inbox rows preloaded right at launch. */
  preloadFirstRows: number;
  /** Rows below those preloaded next, in the background. */
  preloadMoreRows: number;
};

export const DEFAULT_DEV_TUNING: DevTuning = {
  warmRooms: 3,
  preloadFirstRows: 12,
  preloadMoreRows: 36,
};
const LIMITS: Record<keyof DevTuning, number> = {
  warmRooms: 10,
  preloadFirstRows: 50,
  preloadMoreRows: 50,
};

export function parseDevTuning(raw: string | null | undefined): DevTuning {
  if (!raw) return DEFAULT_DEV_TUNING;
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    console.warn("Ignoring malformed dev-tuning.json");
    return DEFAULT_DEV_TUNING;
  }
  const fields =
    parsed && typeof parsed === "object" && !Array.isArray(parsed)
      ? (parsed as Record<string, unknown>)
      : {};
  const read = (key: keyof DevTuning) => {
    const value = fields[key];
    return typeof value === "number" && Number.isFinite(value)
      ? Math.min(LIMITS[key], Math.max(0, Math.floor(value)))
      : DEFAULT_DEV_TUNING[key];
  };
  return {
    warmRooms: read("warmRooms"),
    preloadFirstRows: read("preloadFirstRows"),
    preloadMoreRows: read("preloadMoreRows"),
  };
}

let current: DevTuning & { loaded: boolean } = {
  ...DEFAULT_DEV_TUNING,
  loaded: false,
};
let loading: Promise<void> | null = null;
const listeners = new Set<() => void>();

/** Start reading the file before the first room opens. */
export function preloadDevTuning(): void {
  void loadDevTuning();
}

function loadDevTuning() {
  loading ??= invokeTauri<string | null>("get_dev_tuning")
    .then((raw) => {
      current = { ...parseDevTuning(raw), loaded: true };
    })
    .catch(() => {
      current = { ...DEFAULT_DEV_TUNING, loaded: true };
    })
    .finally(() => {
      for (const listener of listeners) listener();
    });
  return loading;
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  void loadDevTuning();
  return () => {
    listeners.delete(listener);
  };
}

const SERVER_SNAPSHOT = { ...DEFAULT_DEV_TUNING, loaded: false };

/** The launch-time dev tuning; `loaded` turns true once the file was read. */
export function useDevTuning(): DevTuning & { loaded: boolean } {
  return React.useSyncExternalStore(
    subscribe,
    () => current,
    () => SERVER_SNAPSHOT,
  );
}
