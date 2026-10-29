/**
 * Comparing a window opened from the device store with the relay's answer
 * that replaced it (docs/message-sync.md#store-vs-relay-log). The log is the
 * evidence for later skipping the relay when the store is certain.
 */
import type { RelayEvent } from "@/shared/api/types";

type Row = Pick<RelayEvent, "id" | "created_at">;

export type StoreRelayDifference = {
  /** Rows compared: the stored rows inside the shared time range. */
  compared: number;
  /** In the relay's answer, not in the store's. */
  missing: string[];
  /** In the store's answer, not in the relay's (e.g. deleted since). */
  extra: string[];
};

/**
 * Differences inside the time range both answers cover: from the newer of
 * the two oldest rows up to the store's newest row. Rows that arrived live
 * after the store answered, or that fell off the end of the relay's page,
 * are not differences.
 */
export function storeRelayDifference(
  stored: readonly Row[],
  relay: readonly Row[],
): StoreRelayDifference | null {
  if (stored.length === 0 || relay.length === 0) return null;
  const oldest = (rows: readonly Row[]) =>
    Math.min(...rows.map((row) => row.created_at));
  const from = Math.max(oldest(stored), oldest(relay));
  const to = Math.max(...stored.map((row) => row.created_at));
  const inRange = (rows: readonly Row[]) =>
    new Set(
      rows
        .filter((row) => row.created_at >= from && row.created_at <= to)
        .map((row) => row.id),
    );
  const storedIds = inRange(stored);
  const relayIds = inRange(relay);
  return {
    compared: storedIds.size,
    missing: [...relayIds].filter((id) => !storedIds.has(id)),
    extra: [...storedIds].filter((id) => !relayIds.has(id)),
  };
}

/** One log line per comparison: a match at info, a difference at warn. */
export function logStoreComparison(
  scope: string,
  stored: readonly Row[],
  relay: readonly Row[],
) {
  const difference = storeRelayDifference(stored, relay);
  if (!difference) return;
  const { compared, missing, extra } = difference;
  if (missing.length === 0 && extra.length === 0) {
    console.info(`[message-store] ${scope}: store matches relay (${compared})`);
    return;
  }
  console.warn(`[message-store] ${scope}: store differs from relay`, {
    compared,
    missing: missing.slice(0, 5),
    missingCount: missing.length,
    extra: extra.slice(0, 5),
    extraCount: extra.length,
  });
}
