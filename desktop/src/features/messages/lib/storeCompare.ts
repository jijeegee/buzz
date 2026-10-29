/**
 * Comparing a window opened from the device store with the relay's answer
 * that replaced it (docs/message-sync.md#store-vs-relay-log). The log is the
 * evidence for later skipping the relay when the store is certain.
 */
import type { RelayEvent } from "@/shared/api/types";
import {
  CHANNEL_TIMELINE_CONTENT_KINDS,
  KIND_CHANNEL_THREAD_SUMMARY,
} from "@/shared/constants/kinds";

type Row = Pick<RelayEvent, "id" | "created_at">;
type Event = Pick<
  RelayEvent,
  "id" | "created_at" | "kind" | "tags" | "content"
>;

export type StoreRelayDifference = {
  /** Rows compared: the stored rows inside the shared time range. */
  compared: number;
  /** In the relay's answer, not in the store's. */
  missing: string[];
  /** In the store's answer, not in the relay's (e.g. deleted since). */
  extra: string[];
};

export type StoreRelayOverlayDifference = {
  /** Edits, reactions and removals of rows both answers hold. */
  overlaysMissing: string[];
  overlaysExtra: string[];
  /** Shared thread roots whose reply count differs. */
  summaries: string[];
};

const TIMELINE_KINDS = new Set<number>(CHANNEL_TIMELINE_CONTENT_KINDS);

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

/** A thread's reply count as one answer saw it, and when that was. */
export type ReplyCount = { count: number; at: number };

const DELETION_KINDS = new Set([5, 9005]);

/** The newest summary's `reply_count` per root, from 39005 events. */
export function replyCountsOf(events: readonly Event[]) {
  const byRoot = new Map<string, ReplyCount>();
  for (const event of events) {
    if (event.kind !== KIND_CHANNEL_THREAD_SUMMARY) continue;
    const root = event.tags.find((tag) => tag[0] === "e")?.[1];
    if (!root) continue;
    let count: number;
    try {
      count = Number(JSON.parse(event.content).reply_count);
    } catch {
      continue;
    }
    if (!Number.isFinite(count)) continue;
    const kept = byRoot.get(root);
    if (!kept || kept.at < event.created_at) {
      byRoot.set(root, { count, at: event.created_at });
    }
  }
  return byRoot;
}

/**
 * What both answers say about the rows they share: the other events aimed
 * at them (`e` tag) and their thread reply counts. Only what both could
 * know is compared: nothing newer than the store's newest event (`fence`),
 * and no overlay that its own answer also removes (the relay leaves those
 * out). Counts, not whole summaries: a reader's blocks change the rest.
 */
export function storeRelayOverlayDifference(
  stored: readonly Event[],
  relay: readonly Event[],
  shared: ReadonlySet<string>,
  storedCounts: ReadonlyMap<string, ReplyCount>,
  relayCounts: ReadonlyMap<string, ReplyCount>,
  fence: number,
): StoreRelayOverlayDifference {
  const overlays = (events: readonly Event[]) => {
    const removed = new Set(
      events
        .filter((event) => DELETION_KINDS.has(event.kind))
        .flatMap((event) =>
          event.tags.filter((tag) => tag[0] === "e").map((tag) => tag[1]),
        ),
    );
    return new Set(
      events
        .filter(
          (event) =>
            event.created_at <= fence &&
            !removed.has(event.id) &&
            !TIMELINE_KINDS.has(event.kind) &&
            event.kind !== KIND_CHANNEL_THREAD_SUMMARY &&
            event.tags.some(
              (tag) => tag[0] === "e" && shared.has(tag[1] ?? ""),
            ),
        )
        .map((event) => event.id),
    );
  };
  const storedOverlays = overlays(stored);
  const relayOverlays = overlays(relay);
  const roots = new Set(
    [...storedCounts.keys(), ...relayCounts.keys()].filter((root) =>
      shared.has(root),
    ),
  );
  return {
    overlaysMissing: [...relayOverlays].filter((id) => !storedOverlays.has(id)),
    overlaysExtra: [...storedOverlays].filter((id) => !relayOverlays.has(id)),
    summaries: [...roots].filter((root) => {
      const kept = storedCounts.get(root);
      const fresh = relayCounts.get(root);
      if ((fresh?.at ?? 0) > fence || (kept?.at ?? 0) > fence) return false;
      return (kept?.count ?? 0) !== (fresh?.count ?? 0);
    }),
  };
}

type Tally = { matches: number; differs: number };
const TALLY_EVERY_MS = 60_000;
let tally = {
  room: { matches: 0, differs: 0 } as Tally,
  thread: { matches: 0, differs: 0 } as Tally,
  byType: { missing: 0, extra: 0, overlays: 0, summaries: 0 },
};
let tallyLoggedAt: number | null = null;

/** This run's comparison counts (tests and the periodic log). */
export function storeComparisonTally() {
  return tally;
}

/** Start the counts again (tests). */
export function resetStoreComparisonTally() {
  tally = {
    room: { matches: 0, differs: 0 },
    thread: { matches: 0, differs: 0 },
    byType: { missing: 0, extra: 0, overlays: 0, summaries: 0 },
  };
  tallyLoggedAt = null;
}

/**
 * One log line per comparison: a match at info, a difference at warn; and
 * this run's counts at most once a minute. `stored` and `relay` are whole
 * answers: timeline rows plus what is aimed at them.
 */
export function logStoreComparison(
  scope: string,
  stream: "room" | "thread",
  stored: readonly Event[],
  relay: readonly Event[],
  now = Date.now(),
  relayCounts?: ReadonlyMap<string, ReplyCount>,
) {
  const rows = (events: readonly Event[]) =>
    events.filter((event) => TIMELINE_KINDS.has(event.kind));
  const difference = storeRelayDifference(rows(stored), rows(relay));
  if (!difference) return;
  const { compared, missing, extra } = difference;
  const relayIds = new Set(relay.map((event) => event.id));
  const shared = new Set(
    rows(stored)
      .map((event) => event.id)
      .filter((id) => relayIds.has(id)),
  );
  const { overlaysMissing, overlaysExtra, summaries } =
    storeRelayOverlayDifference(
      stored,
      relay,
      shared,
      replyCountsOf(stored),
      relayCounts ?? replyCountsOf(relay),
      Math.max(...stored.map((event) => event.created_at)),
    );
  const overlays = overlaysMissing.length + overlaysExtra.length;
  const same =
    missing.length === 0 &&
    extra.length === 0 &&
    overlays === 0 &&
    summaries.length === 0;
  tally[stream][same ? "matches" : "differs"] += 1;
  tally.byType.missing += missing.length;
  tally.byType.extra += extra.length;
  tally.byType.overlays += overlays;
  tally.byType.summaries += summaries.length;
  if (same) {
    console.info(`[message-store] ${scope}: store matches relay (${compared})`);
  } else {
    console.warn(`[message-store] ${scope}: store differs from relay`, {
      compared,
      missing: missing.slice(0, 5),
      missingCount: missing.length,
      extra: extra.slice(0, 5),
      extraCount: extra.length,
      overlaysMissing: overlaysMissing.slice(0, 5),
      overlaysExtra: overlaysExtra.slice(0, 5),
      summaries: summaries.slice(0, 5),
    });
  }
  // The first counts a minute after the first comparison, then each minute.
  if (tallyLoggedAt === null) {
    tallyLoggedAt = now;
  } else if (now - tallyLoggedAt >= TALLY_EVERY_MS) {
    tallyLoggedAt = now;
    console.info("[message-store] comparisons this run", tally);
  }
}
