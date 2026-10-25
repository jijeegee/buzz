import type { QueryClient } from "@tanstack/react-query";

import {
  getMessageStoreCoverage,
  loadMessageStoreHeads,
  type MessageStoreRoomCoverage,
  type MessageStoreScope,
  type MessageStoreWindowPage,
  readMessageStoreWindow,
} from "@/shared/api/tauriMessageStore";
import type { Channel, RelayEvent } from "@/shared/api/types";
import { reconcileChannelWindowMessages } from "./channelWindowReconciliation";
import { parseLiveThreadSummary } from "./channelWindowResponse";
import {
  type ChannelWindowCursor,
  type ChannelWindowPage,
  type ChannelWindowStore,
  emptyChannelWindowStore,
  replaceNewestChannelWindow,
} from "./channelWindowStore";
import { channelMessagesKey, channelWindowKey } from "./messageQueryKeys";

/** Rooms and rows prefilled from the local store at boot. */
const BOOT_HEAD_ROOMS = 16;
export const STORE_HEAD_ROWS = 50;

/** What the store proved about one room's newest page when it was read. */
export type StoreHeadProof = {
  /** Newest time the `top` coverage reaches (null when none). */
  coveredThrough: number | null;
  /** An open live segment continues the newest range. */
  liveCovered: boolean;
  /** The page stopped at the lower edge of coverage before its limit. */
  gap: boolean;
};

const scopes = new WeakMap<QueryClient, MessageStoreScope>();
const hydrations = new WeakMap<QueryClient, Promise<void>>();
/** Seeded heads not yet consumed by the room's first query run. */
const seededChannels = new WeakMap<QueryClient, Map<string, StoreHeadProof>>();
/** Seeded heads, kept for the paint decision after the query consumed them. */
const seededProofs = new WeakMap<QueryClient, Map<string, StoreHeadProof>>();
/** Rooms whose newest page came from the server in the background. */
const paintableChannels = new WeakMap<QueryClient, Set<string>>();

/** Rooms being refreshed after a (re)subscribe: the server answers them. */
const serverRefreshes = new WeakMap<QueryClient, Map<string, number>>();

/**
 * Run `refresh` with the room's query answered by the server, never the
 * store. A subscribe or reconnect refresh closes the interval before the
 * live subscription started, including thread summaries (39005) the store
 * does not sync, so even live coverage cannot prove it.
 */
export async function withServerRefresh<T>(
  queryClient: QueryClient,
  channelId: string,
  refresh: () => Promise<T>,
): Promise<T> {
  let counts = serverRefreshes.get(queryClient);
  if (!counts) {
    counts = new Map();
    serverRefreshes.set(queryClient, counts);
  }
  counts.set(channelId, (counts.get(channelId) ?? 0) + 1);
  try {
    return await refresh();
  } finally {
    const left = (counts.get(channelId) ?? 1) - 1;
    if (left > 0) counts.set(channelId, left);
    else counts.delete(channelId);
  }
}

function isServerRefresh(queryClient: QueryClient, channelId: string) {
  return serverRefreshes.get(queryClient)?.has(channelId) ?? false;
}

export function storeScopeFor(
  queryClient: QueryClient,
): MessageStoreScope | null {
  return scopes.get(queryClient) ?? null;
}

const toSeconds = (value: string | null) => {
  if (value === null) return null;
  const ms = Date.parse(value);
  return Number.isFinite(ms) ? Math.floor(ms / 1000) : null;
};

export function storeHeadProof(
  coverage: MessageStoreRoomCoverage | undefined,
  page: Pick<MessageStoreWindowPage, "gap">,
): StoreHeadProof {
  const top = (coverage?.ranges ?? []).filter((range) => range.class === "top");
  return {
    coveredThrough: top.length
      ? Math.max(...top.map((range) => range.hiAt))
      : null,
    liveCovered: coverage?.liveCovered ?? false,
    gap: page.gap,
  };
}

/**
 * A stored newest page may paint with no server request only when it is the
 * room's current head: it did not stop at a coverage gap, and either a live
 * segment continues it or its coverage reaches the room's last message. An
 * unknown last message time proves nothing.
 */
export function isProvenStoreHead(
  proof: StoreHeadProof,
  lastMessageAt: string | null,
): boolean {
  if (proof.gap) return false;
  if (proof.liveCovered) return true;
  const last = toSeconds(lastMessageAt);
  return (
    last !== null &&
    proof.coveredThrough !== null &&
    proof.coveredThrough >= last
  );
}

/** `created_at` of the stored 39005 behind each mapped page's summaries. */
const storedSummaryTimes = new WeakMap<
  ChannelWindowPage,
  Map<string, number>
>();

/** Map a store page onto the window-store page shape. */
export function storeWindowPage(
  page: MessageStoreWindowPage,
  startCursor: ChannelWindowCursor | null,
): ChannelWindowPage {
  const summaries = new Map<
    string,
    ChannelWindowPage["rows"][number]["thread"]
  >();
  const summaryTimes = new Map<string, number>();
  for (const event of page.summaries) {
    const parsed = parseLiveThreadSummary(event);
    if (parsed) {
      summaries.set(parsed.rootId, parsed.live.summary);
      summaryTimes.set(parsed.rootId, parsed.live.createdAt);
    }
  }
  // A gap means the server holds what lies below: keep paging from the
  // last stored row (the next store read reaches the gap and asks the server).
  const nextCursor = page.gap
    ? (page.nextCursor ?? startCursor)
    : page.nextCursor;
  const mapped: ChannelWindowPage = {
    startCursor,
    rows: page.rows.map((event) => ({
      event,
      thread: summaries.get(event.id) ?? null,
    })),
    aux: page.aux,
    nextCursor,
    hasMore: nextCursor !== null,
  };
  storedSummaryTimes.set(mapped, summaryTimes);
  return mapped;
}

/**
 * Live 39005 summaries a stored page cannot supersede: the store keeps only
 * the summaries server windows carried, so a live one stays unless the
 * page's stored summary for that root is at least as new.
 */
function keptLiveSummaries(
  current: ChannelWindowStore,
  page: ChannelWindowPage,
): ChannelWindowStore["liveSummaries"] {
  const stored = storedSummaryTimes.get(page) ?? new Map<string, number>();
  return Object.fromEntries(
    Object.entries(current.liveSummaries).filter(
      ([rootId, live]) => live.createdAt > (stored.get(rootId) ?? -1),
    ),
  );
}

/**
 * Make `page` the room's newest window and return the rendered timeline for
 * it. The timeline cache is written only when `updatedAt` is given (seeding);
 * a query function returns the timeline instead.
 */
export function applyStoreHead(
  queryClient: QueryClient,
  channelId: string,
  page: ChannelWindowPage,
  updatedAt?: number,
): RelayEvent[] {
  const windowKey = channelWindowKey(channelId);
  const messagesKey = channelMessagesKey(channelId);
  // Merge, don't replace: live events may already sit on this store.
  const current =
    queryClient.getQueryData<ChannelWindowStore>(windowKey) ??
    emptyChannelWindowStore();
  const window = {
    ...replaceNewestChannelWindow(current, page),
    liveSummaries: keptLiveSummaries(current, page),
  };
  const messages = reconcileChannelWindowMessages(
    window,
    queryClient.getQueryData<RelayEvent[]>(messagesKey) ?? [],
  );
  if (updatedAt === undefined) {
    queryClient.setQueryData(windowKey, window);
  } else {
    queryClient.setQueryData(windowKey, window, { updatedAt });
    queryClient.setQueryData(messagesKey, messages, { updatedAt });
  }
  return messages;
}

/**
 * Read a room's newest page from the store; resolves to the page only when
 * the store proves it is the room's current head (see `isProvenStoreHead`).
 */
export async function readProvenStoreHead(
  queryClient: QueryClient,
  channelId: string,
  lastMessageAt: string | null,
): Promise<ChannelWindowPage | null> {
  const scope = storeScopeFor(queryClient);
  if (!scope) return null;
  try {
    const [page, coverage] = await Promise.all([
      readMessageStoreWindow(scope, channelId, null, STORE_HEAD_ROWS),
      getMessageStoreCoverage(scope, [channelId]),
    ]);
    const proof = storeHeadProof(coverage[0], page);
    if (!isProvenStoreHead(proof, lastMessageAt)) return null;
    return storeWindowPage(page, null);
  } catch (error) {
    console.warn("Failed to read the stored room head", channelId, error);
    return null;
  }
}

/**
 * The room query's local answer: its seeded head (once, waiting for the boot
 * seed so a room opened during boot takes it), else a store read, either one
 * only when proven current. Null means the server must answer.
 */
export async function storedChannelMessages(
  queryClient: QueryClient,
  channel: Pick<Channel, "id" | "lastMessageAt">,
): Promise<RelayEvent[] | null> {
  const lastMessageAt = channel.lastMessageAt;
  await channelHeadHydration(queryClient);
  if (isServerRefresh(queryClient, channel.id)) {
    // Drop any unconsumed seed: the server's page supersedes it.
    consumeHydratedChannel(queryClient, channel.id, null);
    return null;
  }
  if (consumeHydratedChannel(queryClient, channel.id, lastMessageAt)) {
    return (
      queryClient.getQueryData<RelayEvent[]>(channelMessagesKey(channel.id)) ??
      []
    );
  }
  const stored = await readProvenStoreHead(
    queryClient,
    channel.id,
    lastMessageAt,
  );
  return stored ? applyStoreHead(queryClient, channel.id, stored) : null;
}

/**
 * Resolves once stored heads have been seeded into this client (or at once
 * when none was started). The room query awaits it so a room opened during
 * boot takes the seeded path instead of racing it with a server fetch.
 */
export function channelHeadHydration(queryClient: QueryClient): Promise<void> {
  return hydrations.get(queryClient) ?? Promise.resolve();
}

/**
 * The room query's one use of a seeded head: true when the seed is proven
 * for this room now, so the query keeps it without asking anyone.
 */
export function consumeHydratedChannel(
  queryClient: QueryClient,
  channelId: string,
  lastMessageAt: string | null,
): boolean {
  const channels = seededChannels.get(queryClient);
  const proof = channels?.get(channelId);
  if (!channels || !proof) return false;
  channels.delete(channelId);
  if (channels.size === 0) seededChannels.delete(queryClient);
  return isProvenStoreHead(proof, lastMessageAt);
}

/**
 * Whether the cached rows may paint while the room query settles: a page the
 * server delivered in the background, or a stored head proven current.
 */
export function hasPersistedHydratedChannel(
  queryClient: QueryClient,
  channelId: string,
  lastMessageAt: string | null = null,
): boolean {
  if (paintableChannels.get(queryClient)?.has(channelId)) return true;
  const proof = seededProofs.get(queryClient)?.get(channelId);
  return proof !== undefined && isProvenStoreHead(proof, lastMessageAt);
}

/** The room's cached rows were seeded from the store at boot. */
export function isStoreSeededChannel(
  queryClient: QueryClient,
  channelId: string,
): boolean {
  return seededProofs.get(queryClient)?.has(channelId) ?? false;
}

/**
 * Lets a room whose page was loaded from the server in the background paint
 * that page while it revalidates on open.
 */
export function markPaintableChannelSnapshot(
  queryClient: QueryClient,
  channelId: string,
): void {
  const channels = paintableChannels.get(queryClient);
  if (channels) {
    channels.add(channelId);
  } else {
    paintableChannels.set(queryClient, new Set([channelId]));
  }
}

export function hydrateChannelHeads(
  queryClient: QueryClient,
  scope: MessageStoreScope,
): Promise<void> {
  scopes.set(queryClient, scope);
  const hydration = seedChannelHeads(queryClient, scope).catch((error) => {
    console.warn("Failed to seed stored room heads", error);
  });
  hydrations.set(queryClient, hydration);
  return hydration;
}

async function seedChannelHeads(
  queryClient: QueryClient,
  scope: MessageStoreScope,
): Promise<void> {
  const heads = await loadMessageStoreHeads(
    scope,
    BOOT_HEAD_ROOMS,
    STORE_HEAD_ROWS,
  );
  // A head that stops at a gap is never painted; skip its coverage read.
  const candidates = heads.filter(
    (head) => !head.page.gap && head.page.rows.length > 0,
  );
  if (candidates.length === 0) return;
  const coverage = await getMessageStoreCoverage(
    scope,
    candidates.map((head) => head.channelId),
  );
  const byRoom = new Map(coverage.map((room) => [room.channelId, room]));
  const seeded = new Map<string, StoreHeadProof>();
  for (const head of candidates) {
    try {
      const page = storeWindowPage(head.page, null);
      applyStoreHead(queryClient, head.channelId, page, 0);
      seeded.set(
        head.channelId,
        storeHeadProof(byRoom.get(head.channelId), head.page),
      );
    } catch (error) {
      console.warn("Ignoring an invalid stored head", head.channelId, error);
    }
  }
  if (seeded.size > 0) {
    seededChannels.set(queryClient, seeded);
    seededProofs.set(queryClient, new Map(seeded));
  }
}
