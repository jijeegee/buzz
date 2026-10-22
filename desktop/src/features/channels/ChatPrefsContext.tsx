import * as React from "react";

/**
 * Per-person room preferences the room info panel edits: notification mute
 * (kind:30078 `d=channel-mutes`) and private chat names (`d=channel-names`).
 * AppShell owns both syncs and publishes them here with
 * {@link ChatPrefsPublisher}.
 */
export type ChatPrefs = {
  mutedChannelIds: ReadonlySet<string>;
  muteChannel: (channelId: string) => void;
  unmuteChannel: (channelId: string) => void;
  /** Set (name) or restore (null) your private name for a chat. */
  setPrivateChatName: (channelId: string, name: string | null) => Promise<void>;
};

const EMPTY: ReadonlySet<string> = new Set();

let current: ChatPrefs | null = null;
const listeners = new Set<() => void>();

function publish(next: ChatPrefs | null) {
  current = next;
  for (const listener of listeners) listener();
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** Renders nothing; makes `value` readable through {@link useChatPrefs}. */
export function ChatPrefsPublisher({ value }: { value: ChatPrefs }) {
  React.useLayoutEffect(() => {
    publish(value);
  }, [value]);
  React.useLayoutEffect(() => () => publish(null), []);
  return null;
}

/** Null outside AppShell (tests, secondary windows): controls hide. */
export function useChatPrefs(): ChatPrefs | null {
  return React.useSyncExternalStore(
    subscribe,
    () => current,
    () => null,
  );
}

export function useIsChannelMuted(channelId: string | null): boolean {
  const prefs = useChatPrefs();
  return channelId ? (prefs?.mutedChannelIds ?? EMPTY).has(channelId) : false;
}

/** Test seam: publish prefs without rendering AppShell. */
export function setChatPrefsForTests(next: ChatPrefs | null) {
  publish(next);
}
