/** Unread state of one watched account. */
export type WatchedUnread = { hasUnread: boolean; mentionCount: number };

type Listener = () => void;

/**
 * Unread dot / mention count per watched account, keyed `origin|pubkey`.
 * Read with `useSyncExternalStore(subscribe, snapshot)`.
 */
export function createWatchedUnreadStore() {
  let state: ReadonlyMap<string, WatchedUnread> = new Map();
  const listeners = new Set<Listener>();
  const publish = (next: Map<string, WatchedUnread>) => {
    state = next;
    for (const listener of listeners) listener();
  };
  return {
    subscribe(listener: Listener): () => void {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    snapshot(): ReadonlyMap<string, WatchedUnread> {
      return state;
    },
    set(key: string, value: WatchedUnread): void {
      const current = state.get(key);
      if (
        current?.hasUnread === value.hasUnread &&
        current.mentionCount === value.mentionCount
      )
        return;
      publish(new Map(state).set(key, value));
    },
    bump(key: string, mention: boolean): void {
      const current = state.get(key) ?? { hasUnread: false, mentionCount: 0 };
      this.set(key, {
        hasUnread: true,
        mentionCount: current.mentionCount + (mention ? 1 : 0),
      });
    },
    remove(key: string): void {
      if (!state.has(key)) return;
      const next = new Map(state);
      next.delete(key);
      publish(next);
    },
  };
}

export type WatchedUnreadStore = ReturnType<typeof createWatchedUnreadStore>;

export const watchedUnreadStore = createWatchedUnreadStore();

export function watchKey(origin: string, pubkey: string): string {
  return `${origin}|${pubkey.toLowerCase()}`;
}
