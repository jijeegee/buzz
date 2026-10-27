import type { RelayEvent } from "@/shared/api/types";
import type {
  LiveSubscriptionReadiness,
  RelaySubscriptionFilter,
} from "@/shared/api/relayClientShared";

/**
 * Kinds the account's own documents and signals use: settings documents
 * (30078 by `d`/`t` tag), contact list, deletions, agent personas, teams and
 * catalogs, and event reminders. Shared with mobile through
 * `test-fixtures/me-live-bundle.json` (docs/message-sync.md).
 */
export const ME_AUTHORED_KINDS: readonly number[] = [
  3, 5, 30078, 30175, 30176, 30177, 30178, 30182, 30300,
];

/** Kinds addressed to the account: mentions, approvals, reminders, chat-list pings, sign-in and membership notices. */
export const ME_ADDRESSED_KINDS: readonly number[] = [
  1, 9, 24210, 24211, 40002, 40007, 43001, 43002, 43003, 43004, 43005, 43006,
  44100, 44101, 45001, 45003, 46010, 46011, 46012,
];

/** Filter keys a bundle listener may use; anything else needs its own REQ. */
const LISTENER_KEYS = new Set([
  "kinds",
  "authors",
  "#p",
  "#d",
  "#t",
  "since",
  "limit",
]);
const CATCH_UP_LIMIT = 500;
const LISTENER_SEEN_LIMIT = 2_000;
/** Keeps a scope open across an unmount/remount of its last listener. */
const CLOSE_DELAY_MS = 1_000;
const CATCH_UP_ATTEMPTS = 3;
const CATCH_UP_RETRY_MS = 1_000;

export type MeScope = { kind: "authored" | "addressed"; pubkey: string };

/**
 * The bundle a filter can ride: one pubkey in `authors` (authored) or in `#p`
 * (addressed), never both, only kinds that bundle carries, and only the keys
 * above. Anything else returns null.
 */
export function meScopeOf(filter: RelaySubscriptionFilter): MeScope | null {
  if (Object.keys(filter).some((key) => !LISTENER_KEYS.has(key))) return null;
  const kinds = filter.kinds;
  if (!kinds || kinds.length === 0) return null;
  const authors = filter.authors;
  const addressed = filter["#p"];
  if (authors && !addressed && authors.length === 1) {
    if (!kinds.every((kind) => ME_AUTHORED_KINDS.includes(kind))) return null;
    return { kind: "authored", pubkey: authors[0].toLowerCase() };
  }
  if (addressed && !authors && addressed.length === 1) {
    if (!kinds.every((kind) => ME_ADDRESSED_KINDS.includes(kind))) return null;
    return { kind: "addressed", pubkey: addressed[0].toLowerCase() };
  }
  return null;
}

/**
 * Whether a live event reaches a listener with `filter`. `since` and `limit`
 * shape only the listener's history, not live delivery: the old per-feature
 * REQ dropped live events stamped before `since`, which only lost events.
 */
export function eventMatchesFilter(
  event: RelayEvent,
  filter: RelaySubscriptionFilter,
) {
  if (filter.kinds && !filter.kinds.includes(event.kind)) return false;
  if (
    filter.authors &&
    !filter.authors.some(
      (author) => author.toLowerCase() === event.pubkey.toLowerCase(),
    )
  ) {
    return false;
  }
  for (const [key, values] of Object.entries(filter)) {
    if (!key.startsWith("#") || !Array.isArray(values)) continue;
    const name = key.slice(1);
    const wanted = new Set(
      (values as string[]).map((value) =>
        name === "p" ? value.toLowerCase() : value,
      ),
    );
    const hit = event.tags.some(
      (tag) =>
        tag[0] === name &&
        typeof tag[1] === "string" &&
        wanted.has(name === "p" ? tag[1].toLowerCase() : tag[1]),
    );
    if (!hit) return false;
  }
  return true;
}

export function meScopeFilter(scope: MeScope): RelaySubscriptionFilter {
  return scope.kind === "authored"
    ? { kinds: [...ME_AUTHORED_KINDS], authors: [scope.pubkey], limit: 0 }
    : { kinds: [...ME_ADDRESSED_KINDS], "#p": [scope.pubkey], limit: 0 };
}

type Listener = {
  filter: RelaySubscriptionFilter;
  onEvent: (event: RelayEvent) => void;
  seen: Map<string, true>;
};

type ScopeEntry = {
  scope: MeScope;
  listeners: Set<Listener>;
  ready: Promise<LiveSubscriptionReadiness>;
  live: boolean;
  /** Start of a window a failed catch-up still owes; retried next reconnect. */
  owedSince: number | null;
  dispose: (() => Promise<void>) | null;
  closeTimer: number | null;
  controller: AbortController;
};

export type MeBundleDependencies = {
  open: (
    filter: RelaySubscriptionFilter,
    onEvent: (event: RelayEvent) => void,
    onReady: (readiness: LiveSubscriptionReadiness) => void,
    signal: AbortSignal,
  ) => Promise<() => Promise<void>>;
  fetch: (filter: RelaySubscriptionFilter) => Promise<RelayEvent[]>;
  setTimeout: (fn: () => void, ms: number) => number;
  clearTimeout: (id: number) => void;
};

function deliver(listener: Listener, event: RelayEvent) {
  if (listener.seen.has(event.id)) return;
  if (!eventMatchesFilter(event, listener.filter)) return;
  listener.seen.set(event.id, true);
  if (listener.seen.size > LISTENER_SEEN_LIMIT) {
    const oldest = listener.seen.keys().next().value;
    if (oldest !== undefined) listener.seen.delete(oldest);
  }
  listener.onEvent(event);
}

function byCreatedAt(a: RelayEvent, b: RelayEvent) {
  return a.created_at - b.created_at;
}

/**
 * Every live subscription about the account on two REQs: what it authored
 * (`authors: [me]`) and what is addressed to it (`#p: [me]`). Features keep
 * their filters; the bundle opens one `limit: 0` REQ per scope with every
 * kind that scope carries and hands each listener the events its filter
 * matches, once.
 *
 * A listener whose filter asks for history (a nonzero `limit`, from its
 * `since` if any) gets it fetched for it alone once the scope is live, as its
 * own REQ's history used to arrive. After a
 * reconnect the bundle fetches what each scope missed while the connection
 * was down (`catchUp`); listeners drop events they already had.
 */
export class MeLiveBundle {
  private scopes = new Map<string, ScopeEntry>();
  private generation = 0;
  private readonly deps: MeBundleDependencies;

  constructor(deps: MeBundleDependencies) {
    this.deps = deps;
  }

  /**
   * Listen with `filter` (see `meScopeOf`). Resolves once the scope's REQ is
   * live and any history the filter asks for has been delivered, with the
   * stop function;
   * rejects if either fails, leaving nothing registered.
   */
  async subscribe(
    filter: RelaySubscriptionFilter,
    onEvent: (event: RelayEvent) => void,
    onReady?: (readiness: LiveSubscriptionReadiness) => void,
  ): Promise<() => Promise<void>> {
    const scope = meScopeOf(filter);
    if (!scope) throw new Error("Filter is not scoped to one account.");
    const key = `${scope.kind}:${scope.pubkey}`;
    const entry = this.scopes.get(key) ?? this.openScope(key, scope);
    if (entry.closeTimer !== null) {
      this.deps.clearTimeout(entry.closeTimer);
      entry.closeTimer = null;
    }
    const listener: Listener = { filter, onEvent, seen: new Map() };
    entry.listeners.add(listener);

    let stopped = false;
    const stop = async () => {
      if (stopped) return;
      stopped = true;
      entry.listeners.delete(listener);
      if (entry.listeners.size > 0 || this.scopes.get(key) !== entry) return;
      entry.closeTimer = this.deps.setTimeout(() => {
        entry.closeTimer = null;
        if (entry.listeners.size > 0 || this.scopes.get(key) !== entry) return;
        this.scopes.delete(key);
        this.closeScope(entry);
      }, CLOSE_DELAY_MS);
    };

    try {
      const readiness = await entry.ready;
      if (filter.limit > 0 && !stopped) {
        const history = await this.deps.fetch({
          ...filter,
          limit: Math.min(filter.limit, CATCH_UP_LIMIT),
        });
        if (!stopped) {
          for (const event of [...history].sort(byCreatedAt)) {
            deliver(listener, event);
          }
        }
      }
      onReady?.(readiness);
      return stop;
    } catch (error) {
      await stop();
      throw error;
    }
  }

  /**
   * Fetch what every live scope missed since `sinceSecs` (the moment the
   * connection dropped, minus the relay's created-at skew allowance). A scope
   * whose fetch keeps failing owes that window to the next reconnect.
   */
  async catchUp(sinceSecs: number) {
    const generation = this.generation;
    await Promise.all(
      [...this.scopes.values()]
        .filter((entry) => entry.live)
        .map((entry) => this.catchUpScope(entry, sinceSecs, generation)),
    );
  }

  /** Drop every scope (session teardown, community switch). */
  reset() {
    this.generation += 1;
    for (const entry of this.scopes.values()) this.closeScope(entry);
    this.scopes.clear();
  }

  private openScope(key: string, scope: MeScope) {
    const generation = this.generation;
    const controller = new AbortController();
    const entry: ScopeEntry = {
      scope,
      listeners: new Set(),
      ready: Promise.resolve("eose"),
      live: false,
      owedSince: null,
      dispose: null,
      closeTimer: null,
      controller,
    };
    let readiness: LiveSubscriptionReadiness = "eose";
    entry.ready = this.deps
      .open(
        meScopeFilter(scope),
        (event) => {
          if (this.scopes.get(key) !== entry) return;
          for (const listener of [...entry.listeners]) deliver(listener, event);
        },
        (next) => {
          readiness = next;
        },
        controller.signal,
      )
      .then(
        (dispose) => {
          if (
            generation !== this.generation ||
            this.scopes.get(key) !== entry
          ) {
            void dispose().catch(() => {});
            return readiness;
          }
          entry.dispose = dispose;
          entry.live = true;
          return readiness;
        },
        (error: unknown) => {
          // The next subscriber opens a fresh REQ; waiting ones reject and
          // retry through their own paths, as with a REQ of their own.
          if (this.scopes.get(key) === entry) this.scopes.delete(key);
          throw error;
        },
      );
    // Callers observe the rejection; this keeps an unobserved one quiet.
    entry.ready.catch(() => {});
    this.scopes.set(key, entry);
    return entry;
  }

  private closeScope(entry: ScopeEntry) {
    entry.controller.abort();
    entry.live = false;
    if (entry.closeTimer !== null) {
      this.deps.clearTimeout(entry.closeTimer);
      entry.closeTimer = null;
    }
    void entry.dispose?.().catch(() => {});
  }

  private async catchUpScope(
    entry: ScopeEntry,
    sinceSecs: number,
    generation: number,
  ) {
    const isCurrent = () =>
      generation === this.generation && entry.live && entry.listeners.size > 0;
    const since = Math.max(
      0,
      Math.floor(Math.min(sinceSecs, entry.owedSince ?? sinceSecs)),
    );
    entry.owedSince = since;
    for (let attempt = 1; attempt <= CATCH_UP_ATTEMPTS; attempt += 1) {
      if (!isCurrent()) return;
      try {
        const missed = await this.deps.fetch({
          ...meScopeFilter(entry.scope),
          since,
          limit: CATCH_UP_LIMIT,
        });
        if (!isCurrent()) return;
        entry.owedSince = null;
        for (const event of [...missed].sort(byCreatedAt)) {
          for (const listener of [...entry.listeners]) deliver(listener, event);
        }
        return;
      } catch (error) {
        if (attempt === CATCH_UP_ATTEMPTS) {
          console.error("Failed to catch up account events", error);
          return;
        }
        await new Promise<void>((resolve) => {
          this.deps.setTimeout(resolve, CATCH_UP_RETRY_MS * attempt);
        });
      }
    }
  }
}
