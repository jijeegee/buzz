import * as React from "react";

/**
 * User preference for how a thread opens inside a channel.
 *
 * - `focus` — a large right-anchored drawer overlays the channel content area,
 *   leaving a narrow scrim-dimmed sliver of the channel visible as an
 *   orientation cue and a click-target back to the channel.
 * - `split` — the thread opens in a resizable side panel next to the channel.
 *
 * Persisted in localStorage. This is a device-level UI preference, not
 * community-scoped data, so it is intentionally not reset on community switch.
 * Only applies at viewport widths wide enough for a two-pane channel view;
 * narrow viewports keep their single-panel/floating-overlay behavior.
 */
export type ThreadViewMode = "focus" | "split";

const STORAGE_KEY = "buzz.channels.threadViewMode";

/** Layout used when nothing is stored, or the stored value is unrecognized. */
const DEFAULT_THREAD_VIEW_MODE: ThreadViewMode = "split";

const listeners = new Set<() => void>();

let threadViewMode = readStoredThreadViewMode();

function parseThreadViewMode(value: string | null | undefined): ThreadViewMode {
  return value === "focus" || value === "split"
    ? value
    : DEFAULT_THREAD_VIEW_MODE;
}

function readStoredThreadViewMode(): ThreadViewMode {
  try {
    return parseThreadViewMode(globalThis.localStorage?.getItem(STORAGE_KEY));
  } catch {
    return DEFAULT_THREAD_VIEW_MODE;
  }
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

function getSnapshot(): ThreadViewMode {
  return threadViewMode;
}

function getServerSnapshot(): ThreadViewMode {
  return DEFAULT_THREAD_VIEW_MODE;
}

/** Read the persisted thread layout preference outside of React. */
export function getThreadViewMode(): ThreadViewMode {
  return threadViewMode;
}

/** Update the thread layout preference and notify all subscribed components. */
export function setThreadViewMode(mode: ThreadViewMode): void {
  threadViewMode = mode;

  try {
    globalThis.localStorage?.setItem(STORAGE_KEY, mode);
  } catch {
    // Persistence is best-effort; the in-memory value still applies.
  }

  for (const listener of listeners) {
    listener();
  }
}

// The inbox keeps its own thread layout: threads entered from the inbox open
// maximized by default, and the layout toggle there is remembered on its own,
// across closing the inbox panel and restarts, without touching the channel
// default above.
const INBOX_STORAGE_KEY = "buzz.channels.inboxThreadViewMode";
const DEFAULT_INBOX_THREAD_VIEW_MODE: ThreadViewMode = "focus";

const inboxListeners = new Set<() => void>();

let inboxThreadViewMode = readStoredInboxThreadViewMode();

function readStoredInboxThreadViewMode(): ThreadViewMode {
  try {
    const value = globalThis.localStorage?.getItem(INBOX_STORAGE_KEY);
    return value === "focus" || value === "split"
      ? value
      : DEFAULT_INBOX_THREAD_VIEW_MODE;
  } catch {
    return DEFAULT_INBOX_THREAD_VIEW_MODE;
  }
}

function subscribeInbox(listener: () => void): () => void {
  inboxListeners.add(listener);
  return () => {
    inboxListeners.delete(listener);
  };
}

/** Read the persisted inbox thread layout outside of React. */
export function getInboxThreadViewMode(): ThreadViewMode {
  return inboxThreadViewMode;
}

/** Update the inbox thread layout and notify all subscribed components. */
export function setInboxThreadViewMode(mode: ThreadViewMode): void {
  inboxThreadViewMode = mode;

  try {
    globalThis.localStorage?.setItem(INBOX_STORAGE_KEY, mode);
  } catch {
    // Persistence is best-effort; the in-memory value still applies.
  }

  for (const listener of inboxListeners) {
    listener();
  }
}

/** The remembered layout for threads entered from the inbox. */
export function useInboxThreadViewMode(): ThreadViewMode {
  return React.useSyncExternalStore(
    subscribeInbox,
    () => inboxThreadViewMode,
    () => DEFAULT_INBOX_THREAD_VIEW_MODE,
  );
}

type ThreadViewModeOverride = {
  mode: ThreadViewMode;
  setMode: (mode: ThreadViewMode) => void;
};

const ThreadViewModeOverrideContext =
  React.createContext<ThreadViewModeOverride | null>(null);

/**
 * Scopes the thread layout to one surface without touching the channel
 * preference: the inbox uses its own remembered layout, and its toggle only
 * switches that inbox layout.
 */
export const ThreadViewModeOverrideProvider =
  ThreadViewModeOverrideContext.Provider;

/** How threads should open in a channel: as a focus drawer or a split pane. */
export function useThreadViewMode(): ThreadViewMode {
  const override = React.useContext(ThreadViewModeOverrideContext);
  const stored = React.useSyncExternalStore(
    subscribe,
    getSnapshot,
    getServerSnapshot,
  );
  return override?.mode ?? stored;
}

/** Setter for the layout in effect here: the scoped override or the preference. */
export function useSetThreadViewMode(): (mode: ThreadViewMode) => void {
  return (
    React.useContext(ThreadViewModeOverrideContext)?.setMode ??
    setThreadViewMode
  );
}
