import { DM_NOTIFIABLE_EVENT_KINDS } from "@/features/channels/isDmNotifiableKind";
import {
  type CommunityUnreadObserverResult,
  type CommunityUnreadRelay,
  fetchCommunityUnread,
  fetchObservedChannels,
  type ObservedChannel,
} from "@/features/communities/communityUnreadObserver";
import type { ReadOnlyRelayClientOptions } from "@/shared/api/readOnlyRelayClient";
import type { RelaySubscriptionFilter } from "@/shared/api/relayClientShared";
import type { RelayEvent } from "@/shared/api/types";
import {
  CHANNEL_MESSAGE_EVENT_KINDS,
  KIND_DM_VISIBILITY,
  KIND_MEMBER_ADDED_NOTIFICATION,
  KIND_MEMBER_REMOVED_NOTIFICATION,
  KIND_READ_STATE,
} from "@/shared/constants/kinds";

import {
  classifyWatchedEvent,
  type WatchedAccountContext,
  type WatchedEventEffect,
} from "./classify";
import type { WatchedUnreadStore } from "./store";

/** The relay caps explicit `#h` values per REQ (relay req.rs). */
export const MAX_CHANNELS_PER_REQ = 128;
const SEEN_LIMIT = 500;
/** Stored events replayed per channel chunk after a reconnect. */
const REPLAY_LIMIT = 200;
/** Replayed window on resubscribe, against clock skew and in-flight events. */
const RESUBSCRIBE_OVERLAP_SECONDS = 60;
const BACKOFF_BASE_MS = 1_000;
const BACKOFF_MAX_MS = 60_000;
/** Coalesces bursts of membership / read-state changes into one re-read. */
const REFRESH_DEBOUNCE_MS = 750;

const MESSAGE_KINDS = [
  ...new Set<number>([
    ...CHANNEL_MESSAGE_EVENT_KINDS,
    ...DM_NOTIFIABLE_EVENT_KINDS,
  ]),
];

export type WatchClient = CommunityUnreadRelay & {
  connect(): Promise<void>;
  subscribeLive(
    subId: string,
    filter: RelaySubscriptionFilter,
    onEvent: (event: RelayEvent) => void,
    onClosed?: (message: string) => void,
  ): Promise<void>;
  unsubscribe(subId: string): void;
  /** Final: the client must not reconnect afterwards. */
  close(): void;
};

export type WatcherDeps = {
  createClient(
    relayUrl: string,
    options: ReadOnlyRelayClientOptions,
  ): WatchClient;
  authFrame: NonNullable<ReadOnlyRelayClientOptions["authFrame"]>;
  decrypt(ciphertext: string): Promise<string>;
  readContext(
    pubkey: string,
  ): Omit<WatchedAccountContext, "mutedChannelIds" | "pubkey">;
  notify(
    effect: WatchedEventEffect,
    event: RelayEvent,
    channel: ObservedChannel,
  ): void;
  store: WatchedUnreadStore;
  listChannels?: typeof fetchObservedChannels;
  countUnread?: typeof fetchCommunityUnread;
  now?: () => number;
  random?: () => number;
  setTimer?: (callback: () => void, ms: number) => unknown;
  clearTimer?: (handle: unknown) => void;
  log?: (message: string, error?: unknown) => void;
};

/** Reconnect delay after `attempt` consecutive failures: exponential, jittered. */
export function backoffDelayMs(attempt: number, random: () => number): number {
  const ceiling = Math.min(BACKOFF_MAX_MS, BACKOFF_BASE_MS * 2 ** attempt);
  return Math.round(ceiling * (0.5 + random() * 0.5));
}

export function chunkChannels<T>(items: readonly T[], size: number): T[][] {
  const chunks: T[][] = [];
  for (let index = 0; index < items.length; index += size) {
    chunks.push(items.slice(index, index + size));
  }
  return chunks;
}

/**
 * One long-lived read-only relay session for an account that is signed in on
 * this computer but not open: it subscribes to the account's channels, keeps
 * the subscription in step with membership changes, notifies, and keeps the
 * account's unread state in `store`. Reconnects on its own with backoff.
 */
export class InactiveAccountWatcher {
  private client: WatchClient | null = null;
  private channels = new Map<string, ObservedChannel>();
  private channelSubIds: string[] = [];
  private mutedChannelIds: ReadonlySet<string> = new Set();
  private seen: string[] = [];
  private seenSet = new Set<string>();
  private lastSeen: number | null = null;
  /** `since` of the first subscription; the floor until a message is seen. */
  private subscribedSince: number | null = null;
  private attempt = 0;
  private retryTimer: unknown = null;
  private refreshTimer: unknown = null;
  private running = false;
  private connecting = false;
  private stopped = false;
  private readonly pubkey: string;

  readonly key: string;
  readonly relayUrl: string;
  private readonly deps: WatcherDeps;

  constructor(
    key: string,
    relayUrl: string,
    pubkey: string,
    deps: WatcherDeps,
  ) {
    this.key = key;
    this.relayUrl = relayUrl;
    this.pubkey = pubkey.toLowerCase();
    this.deps = deps;
  }

  start(): void {
    if (this.stopped || this.running || this.connecting) return;
    void this.connect();
  }

  /** Reconnect now if the session is down (resume, online, focus). */
  resume(): void {
    if (this.stopped || this.running || this.connecting) return;
    this.clearRetry();
    void this.connect();
  }

  stop(): void {
    this.stopped = true;
    this.clearRetry();
    this.clearRefresh();
    this.dropClient();
    this.deps.store.remove(this.key);
  }

  private async connect(): Promise<void> {
    this.connecting = true;
    const client = this.deps.createClient(this.relayUrl, {
      authFrame: this.deps.authFrame,
      onClose: () => this.handleDrop(client),
    });
    this.client = client;
    try {
      await client.connect();
      const listChannels = this.deps.listChannels ?? fetchObservedChannels;
      const channels = await listChannels(client, this.pubkey);
      if (this.client !== client) return;
      this.setChannels(channels);
      const since = this.resubscribeSince();
      this.subscribedSince ??= since;
      await this.subscribeAccount(client, since);
      await this.subscribeChannels(client, since);
      if (this.client !== client) return;
      this.running = true;
      this.attempt = 0;
      await this.recount(client);
    } catch (error) {
      if (this.client !== client) return;
      this.deps.log?.(`[accounts] watch ${this.key} failed`, error);
      this.handleDrop(client);
    } finally {
      this.connecting = false;
    }
  }

  private handleDrop(client: WatchClient): void {
    if (this.client !== client || this.stopped) return;
    this.running = false;
    this.dropClient();
    const delay = backoffDelayMs(this.attempt, this.deps.random ?? Math.random);
    this.attempt += 1;
    this.clearRetry();
    this.retryTimer = this.setTimer(() => {
      this.retryTimer = null;
      if (!this.stopped) void this.connect();
    }, delay);
  }

  private dropClient(): void {
    const client = this.client;
    this.client = null;
    this.running = false;
    this.channelSubIds = [];
    client?.close();
  }

  private setChannels(channels: ObservedChannel[]): void {
    this.channels = new Map(channels.map((channel) => [channel.id, channel]));
  }

  /** Membership changes and the account's own read state / mutes. */
  private async subscribeAccount(
    client: WatchClient,
    since: number,
  ): Promise<void> {
    await client.subscribeLive(
      "w-mem",
      {
        kinds: [
          KIND_MEMBER_ADDED_NOTIFICATION,
          KIND_MEMBER_REMOVED_NOTIFICATION,
          KIND_DM_VISIBILITY,
        ],
        "#p": [this.pubkey],
        since,
        limit: 50,
      },
      (event) => {
        if (event.created_at >= since) this.scheduleRefresh(true);
      },
    );
    // Read state and channel mutes share this kind: either one re-counts.
    await client.subscribeLive(
      "w-rs",
      {
        kinds: [KIND_READ_STATE],
        authors: [this.pubkey],
        since,
        limit: 20,
      },
      () => this.scheduleRefresh(false),
    );
  }

  /** `#h` subscriptions in relay-sized chunks; extra old chunks are closed. */
  private async subscribeChannels(
    client: WatchClient,
    since: number,
  ): Promise<void> {
    const ids = [...this.channels.keys()];
    const chunks = chunkChannels(ids, MAX_CHANNELS_PER_REQ);
    const subIds = chunks.map((_, index) => `w-ch-${index}`);
    for (const [index, chunk] of chunks.entries()) {
      await client.subscribeLive(
        subIds[index],
        { kinds: MESSAGE_KINDS, "#h": chunk, since, limit: REPLAY_LIMIT },
        (event) => this.handleMessage(event),
      );
    }
    for (const stale of this.channelSubIds.slice(subIds.length)) {
      client.unsubscribe(stale);
    }
    this.channelSubIds = subIds;
  }

  private scheduleRefresh(membership: boolean): void {
    if (this.refreshTimer !== null && !membership) return;
    this.clearRefresh();
    this.refreshTimer = this.setTimer(() => {
      this.refreshTimer = null;
      void this.refresh(membership);
    }, REFRESH_DEBOUNCE_MS);
  }

  private async refresh(membership: boolean): Promise<void> {
    const client = this.client;
    if (!client || !this.running) return;
    try {
      if (membership) {
        const listChannels = this.deps.listChannels ?? fetchObservedChannels;
        const channels = await listChannels(client, this.pubkey);
        if (this.client !== client) return;
        this.setChannels(channels);
        await this.subscribeChannels(client, this.resubscribeSince());
      }
      await this.recount(client);
    } catch (error) {
      this.deps.log?.(`[accounts] watch ${this.key} refresh failed`, error);
    }
  }

  private async recount(client: WatchClient): Promise<void> {
    const countUnread = this.deps.countUnread ?? fetchCommunityUnread;
    const result: CommunityUnreadObserverResult = await countUnread({
      client,
      pubkey: this.pubkey,
      channels: [...this.channels.values()],
      decryptReadState: this.deps.decrypt,
      decryptMutes: this.deps.decrypt,
      onMutedChannelIds: (ids) => {
        this.mutedChannelIds = ids;
      },
    });
    if (this.client !== client) return;
    this.deps.store.set(this.key, result);
  }

  private handleMessage(event: RelayEvent): void {
    if (this.seenSet.has(event.id)) return;
    this.remember(event.id);
    if (this.lastSeen === null || event.created_at > this.lastSeen) {
      this.lastSeen = event.created_at;
    }
    const channelId = event.tags.find((tag) => tag[0] === "h")?.[1];
    const channel = channelId ? this.channels.get(channelId) : undefined;
    if (!channel) return;
    const effect = classifyWatchedEvent(event, channel, {
      ...this.deps.readContext(this.pubkey),
      pubkey: this.pubkey,
      mutedChannelIds: this.mutedChannelIds,
    });
    if (effect.unread) this.deps.store.bump(this.key, effect.mention);
    if (effect.notify) this.deps.notify(effect, event, channel);
  }

  private remember(id: string): void {
    this.seen.push(id);
    this.seenSet.add(id);
    if (this.seen.length > SEEN_LIMIT) {
      const dropped = this.seen.shift();
      if (dropped) this.seenSet.delete(dropped);
    }
  }

  /**
   * Where a (re)subscription starts: a little before the newest message seen,
   * or before the first subscription when none was seen yet, so messages sent
   * while re-subscribing are replayed (seen ids drop the repeats). Only the
   * very first subscription starts at now.
   */
  private resubscribeSince(): number {
    const base = this.lastSeen ?? this.subscribedSince;
    return base === null
      ? this.nowSeconds()
      : base - RESUBSCRIBE_OVERLAP_SECONDS;
  }

  private nowSeconds(): number {
    return Math.floor((this.deps.now ?? Date.now)() / 1_000);
  }

  private setTimer(callback: () => void, ms: number): unknown {
    return (this.deps.setTimer ?? ((cb, delay) => setTimeout(cb, delay)))(
      callback,
      ms,
    );
  }

  private clearRetry(): void {
    if (this.retryTimer === null) return;
    (this.deps.clearTimer ?? ((handle) => clearTimeout(handle as never)))(
      this.retryTimer,
    );
    this.retryTimer = null;
  }

  private clearRefresh(): void {
    if (this.refreshTimer === null) return;
    (this.deps.clearTimer ?? ((handle) => clearTimeout(handle as never)))(
      this.refreshTimer,
    );
    this.refreshTimer = null;
  }
}
