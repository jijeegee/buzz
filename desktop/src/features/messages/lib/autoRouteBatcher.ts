/**
 * Smart routing's per-channel send buffer. The owner often sends one request
 * as several short lines; routing them together costs one model call and
 * lets the router group them. A batch fires {@link AUTO_ROUTE_QUIET_MS}
 * after the channel's last send, waits while a composer in that channel
 * still holds text (the owner is mid-thought), and never waits longer than
 * {@link AUTO_ROUTE_MAX_WAIT_MS} from its first send. A full batch fires at
 * once. Community-scoped: reset by `resetCommunityState()`.
 */
export const AUTO_ROUTE_QUIET_MS = 3_000;
export const AUTO_ROUTE_MAX_WAIT_MS = 20_000;
/** Matches the router's per-call cap (`MAX_BATCH` in Rust). */
export const AUTO_ROUTE_MAX_BATCH = 12;

type Bucket = {
  items: unknown[];
  firstAt: number;
  lastAt: number;
  timer: ReturnType<typeof setTimeout> | null;
  flush: (items: unknown[]) => void;
};

const buckets = new Map<string, Bucket>();
/** Composers (by id) per channel whose draft is not empty. */
const holders = new Map<string, Set<string>>();

function fire(channelId: string) {
  const bucket = buckets.get(channelId);
  if (!bucket) return;
  buckets.delete(channelId);
  if (bucket.timer) clearTimeout(bucket.timer);
  bucket.flush(bucket.items);
}

function schedule(channelId: string) {
  const bucket = buckets.get(channelId);
  if (!bucket) return;
  if (bucket.timer) clearTimeout(bucket.timer);
  const deadline = bucket.firstAt + AUTO_ROUTE_MAX_WAIT_MS;
  const held = (holders.get(channelId)?.size ?? 0) > 0;
  const at = held
    ? deadline
    : Math.min(bucket.lastAt + AUTO_ROUTE_QUIET_MS, deadline);
  bucket.timer = setTimeout(
    () => fire(channelId),
    Math.max(0, at - Date.now()),
  );
}

/** Add a sent message; `flush` receives the batch, oldest first. */
export function enqueueAutoRoute<T>(
  channelId: string,
  item: T,
  flush: (items: T[]) => void,
) {
  const now = Date.now();
  const bucket = buckets.get(channelId) ?? {
    items: [],
    firstAt: now,
    lastAt: now,
    timer: null,
    flush: flush as (items: unknown[]) => void,
  };
  bucket.items.push(item);
  bucket.lastAt = now;
  buckets.set(channelId, bucket);
  if (bucket.items.length >= AUTO_ROUTE_MAX_BATCH) fire(channelId);
  else schedule(channelId);
}

/** A composer's draft in `channelId` became non-empty (`held`) or empty. */
export function holdAutoRouteBatch(
  channelId: string,
  composerId: string,
  held: boolean,
) {
  const set = holders.get(channelId) ?? new Set<string>();
  if (held) set.add(composerId);
  else set.delete(composerId);
  if (set.size > 0) holders.set(channelId, set);
  else holders.delete(channelId);
  schedule(channelId);
}

export function resetAutoRouteBatches() {
  for (const bucket of buckets.values()) {
    if (bucket.timer) clearTimeout(bucket.timer);
  }
  buckets.clear();
  holders.clear();
}
