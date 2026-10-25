/**
 * E2E mock of the native message store (`src-tauri/src/message_store/`).
 * Same rules as the Rust store; the JSON fixtures in
 * `src-tauri/testdata/message_store/` run against both so they cannot drift.
 * Each scope persists to localStorage, so a reload behaves like a restart.
 */
import type {
  MessageStoreAroundPage,
  MessageStoreCursor,
  MessageStoreRange,
  MessageStoreRoomCoverage,
  MessageStoreRoomHead,
  MessageStoreScope,
  MessageStoreThreadHeadPage,
  MessageStoreThreadPage,
  MessageStoreWindowPage,
} from "@/shared/api/tauriMessageStore";
import type { RelayEvent } from "@/shared/api/types";

// Same list as `message_store/mod.rs` (checked against
// `src-tauri/testdata/sync_kinds.json` by `kinds.test.mjs`).
export const MOCK_SYNC_KINDS = [
  5, 7, 9, 9005, 40001, 40002, 40003, 40008, 40099, 43001, 43002, 43003, 43004,
  43005, 43006, 45001, 45003, 48100, 48101, 48102, 48103,
] as const;
const SYNC_KINDS = new Set<number>(MOCK_SYNC_KINDS);
const SYNC_KINDS_VERSION = 1;
const WINDOW_ROW_KINDS = new Set([
  9, 40002, 40008, 40099, 43001, 43002, 43003, 43004, 43005, 43006, 48100,
]);
const AUX_KINDS = new Set([5, 7, 9005, 40003]);
const DELETE_KINDS = new Set([5, 9005]);
const KIND_THREAD_SUMMARY = 39005;
const BOUNDS_KINDS = new Set([39006, 39007]);
const TIME_FLOOR_ID = "~";
const LIVE_HI = Number.POSITIVE_INFINITY;
const ROW_OVERHEAD_BYTES = 200;

type Role = 0 | 1 | 2 | 3;
type Range = {
  loAt: number;
  loId: string;
  loIsStart: boolean;
  hiAt: number;
  kindsVersion: number;
};
export type MockClaim = {
  lo: MessageStoreCursor | null;
  loIsStart: boolean;
  hi: { at: number } | { before: MessageStoreCursor };
};
type StoredRow = {
  id: string;
  channelId: string;
  createdAt: number;
  kind: number;
  role: Role;
  rootId: string | null;
  raw: RelayEvent;
  deleted: boolean;
};
type Segment = {
  id: number;
  wsId: number;
  startedAt: number;
  closedAt: number | null;
  channelIds: string[];
};
type State = {
  events: Record<string, StoredRow>;
  targets: Record<string, string[]>;
  summaries: Record<string, { createdAt: number; payload: RelayEvent }>;
  coverage: Record<string, { top: Range[]; all: Range[] }>;
  threadCoverage: Record<string, Range[]>;
  segments: Segment[];
  roomMeta: Record<
    string,
    { lastOpenedAt: number | null; lastEventAt: number; bytesEst: number }
  >;
  lastFrameAt: number | null;
  /** Catch-up watermark (`message_store::sync`); absent in older saves. */
  watermark?: number | null;
};
type ScopeStorage = Pick<Storage, "getItem" | "setItem" | "removeItem"> | null;

const emptyState = (): State => ({
  events: {},
  targets: {},
  summaries: {},
  coverage: {},
  threadCoverage: {},
  segments: [],
  roomMeta: {},
  lastFrameAt: null,
});

const isHex64 = (value: unknown): value is string =>
  typeof value === "string" && /^[0-9a-fA-F]{64}$/.test(value);
/**
 * The mock relay seeds readable `mock-*` event ids that a real relay never
 * sends (ids are verified hex there). Keep them storable, or a stored window
 * would silently miss those rows while its claim still proves it.
 */
const isMockSeedId = (value: unknown): value is string =>
  typeof value === "string" && /^mock-[\w.-]+$/.test(value);
const hexId = (value: unknown) =>
  isHex64(value) ? value.toLowerCase() : isMockSeedId(value) ? value : null;

/** Position order: positive when `a` is newer (relay order). */
function cmpPos(aAt: number, aId: string, bAt: number, bId: string) {
  if (aAt !== bAt) return aAt < bAt ? -1 : 1;
  if (aId === bId) return 0;
  return aId < bId ? 1 : -1;
}
const cmpStr = (a: string, b: string) => (a < b ? -1 : a > b ? 1 : 0);

function contains(range: Range, at: number, id: string) {
  return (
    at <= range.hiAt &&
    (range.loIsStart || cmpPos(at, id, range.loAt, range.loId) >= 0)
  );
}

function normalize(ranges: Range[]): Range[] {
  const sorted = ranges
    .filter((range) => range.kindsVersion === SYNC_KINDS_VERSION)
    .map((range) => ({ ...range }))
    .sort((a, b) => {
      const lo =
        a.loIsStart || b.loIsStart
          ? Number(b.loIsStart) - Number(a.loIsStart)
          : cmpPos(a.loAt, a.loId, b.loAt, b.loId);
      return lo || a.hiAt - b.hiAt;
    });
  const merged: Range[] = [];
  for (const range of sorted) {
    const last = merged[merged.length - 1];
    if (last && (range.loIsStart || range.loAt <= last.hiAt)) {
      last.hiAt = Math.max(last.hiAt, range.hiAt);
    } else {
      merged.push(range);
    }
  }
  return merged;
}

function applyClaim(existing: Range[], claim: MockClaim): Range[] {
  const ranges = normalize(existing);
  if (!claim.lo && !claim.loIsStart) return ranges;
  const [loAt, loId] = claim.lo
    ? [claim.lo.createdAt, claim.lo.eventId]
    : [0, ""];
  let hiAt: number;
  if ("at" in claim.hi) {
    hiAt = claim.hi.at;
  } else {
    const before = claim.hi.before;
    const touched = ranges.find(
      (range) =>
        !range.loIsStart &&
        range.loAt === before.createdAt &&
        range.loId === before.eventId,
    );
    hiAt = touched ? touched.hiAt : before.createdAt - 1;
  }
  const range = {
    loAt,
    loId,
    loIsStart: claim.loIsStart,
    hiAt,
    kindsVersion: SYNC_KINDS_VERSION,
  };
  if (!range.loIsStart && range.loAt > range.hiAt) return ranges;
  return normalize([...ranges, range]);
}

function withLive(ranges: Range[], since: number | null): Range[] {
  if (since === null) return normalize(ranges);
  return normalize([
    ...ranges,
    {
      loAt: since,
      loId: TIME_FLOOR_ID,
      loIsStart: false,
      hiAt: LIVE_HI,
      kindsVersion: SYNC_KINDS_VERSION,
    },
  ]);
}

type Classified =
  | { type: "skip" }
  | { type: "summary"; rootId: string; channelId: string; createdAt: number }
  | {
      type: "event";
      row: Omit<StoredRow, "raw" | "deleted">;
      targets: string[];
    };

function classify(event: RelayEvent, fallback: string | null): Classified {
  const id = hexId(event.id);
  const pubkey = hexId(event.pubkey);
  const { created_at: createdAt, kind } = event;
  if (
    !id ||
    !pubkey ||
    !Number.isInteger(createdAt) ||
    !Number.isInteger(kind) ||
    kind < 0 ||
    kind > 0xffffffff ||
    BOUNDS_KINDS.has(kind)
  ) {
    return { type: "skip" };
  }
  const tags = (Array.isArray(event.tags) ? event.tags : [])
    .filter(Array.isArray)
    .map((tag) => tag.map((part) => (typeof part === "string" ? part : "")));
  const h = tags.find((tag) => tag[0] === "h")?.[1];
  const channelId = h ? h : fallback;
  if (channelId === null || channelId === undefined) return { type: "skip" };
  const eTags = tags.filter((tag) => tag[0] === "e");
  if (kind === KIND_THREAD_SUMMARY) {
    const rootId = eTags.map((tag) => hexId(tag[1])).find(Boolean);
    return rootId
      ? { type: "summary", rootId, channelId, createdAt }
      : { type: "skip" };
  }
  if (!SYNC_KINDS.has(kind)) return { type: "skip" };
  const base = { id, channelId, createdAt, kind };
  if (AUX_KINDS.has(kind)) {
    const targets: string[] = [];
    for (const tag of eTags) {
      const target = hexId(tag[1]);
      if (target && !targets.includes(target)) targets.push(target);
    }
    return { type: "event", row: { ...base, role: 3, rootId: null }, targets };
  }
  const reply = [...eTags].reverse().find((tag) => tag[3] === "reply");
  const parentId = reply ? hexId(reply[1]) : null;
  if (!parentId) {
    return {
      type: "event",
      row: { ...base, role: 0, rootId: null },
      targets: [],
    };
  }
  const rootTag = eTags.find((tag) => tag[3] === "root");
  const rootId = (rootTag && hexId(rootTag[1])) || parentId;
  const broadcast = tags.some(
    (tag) => tag[0] === "broadcast" && tag[1] === "1",
  );
  return {
    type: "event",
    row: { ...base, role: broadcast ? 2 : 1, rootId },
    targets: [],
  };
}

function blank(raw: RelayEvent): RelayEvent {
  return {
    ...raw,
    content: "",
    tags: raw.tags.filter((tag) => ["h", "e", "broadcast"].includes(tag[0])),
  };
}

/** One scope's store; mirrors `ingest.rs` and `read.rs`. */
export class MockScopeStore {
  private state: State;
  private readonly save: (state: State) => void;

  constructor(state: State, save: (state: State) => void) {
    this.state = state;
    this.save = save;
  }

  private markDeleted(target: string, channelId: string) {
    const row = this.state.events[target];
    if (row && row.channelId === channelId && !row.deleted) {
      row.deleted = true;
      row.raw = blank(row.raw);
    }
  }

  insertEvents(events: RelayEvent[], fallbackChannel: string | null = null) {
    let inserted = 0;
    for (const event of events) {
      const classified = classify(event, fallbackChannel);
      if (classified.type === "summary") {
        const current = this.state.summaries[classified.rootId];
        if (!current || classified.createdAt > current.createdAt) {
          this.state.summaries[classified.rootId] = {
            createdAt: classified.createdAt,
            payload: event,
          };
        }
      }
      if (classified.type !== "event") continue;
      const { row, targets } = classified;
      if (this.state.events[row.id]) continue;
      this.state.events[row.id] = { ...row, raw: event, deleted: false };
      inserted += 1;
      for (const target of targets) {
        const list = this.state.targets[target] ?? [];
        if (!list.includes(row.id)) list.push(row.id);
        this.state.targets[target] = list;
      }
      if (DELETE_KINDS.has(row.kind)) {
        for (const target of targets) this.markDeleted(target, row.channelId);
      } else if (
        (this.state.targets[row.id] ?? []).some((deleteId) => {
          const deleter = this.state.events[deleteId];
          return (
            deleter &&
            DELETE_KINDS.has(deleter.kind) &&
            deleter.channelId === row.channelId
          );
        })
      ) {
        this.markDeleted(row.id, row.channelId);
      }
      const meta = this.state.roomMeta[row.channelId] ?? {
        lastOpenedAt: null,
        lastEventAt: 0,
        bytesEst: 0,
      };
      this.state.roomMeta[row.channelId] = meta;
      meta.lastEventAt = Math.max(meta.lastEventAt, row.createdAt);
      meta.bytesEst += JSON.stringify(event).length + ROW_OVERHEAD_BYTES;
    }
    this.save(this.state);
    return inserted;
  }

  watermark(): number | null {
    return this.state.watermark ?? null;
  }

  advanceWatermark(at: number) {
    if (at > (this.state.watermark ?? Number.NEGATIVE_INFINITY)) {
      this.state.watermark = at;
      this.save(this.state);
    }
  }

  noteFrame(at: number) {
    this.state.lastFrameAt = Math.max(this.state.lastFrameAt ?? at, at);
    this.save(this.state);
  }

  claimRoom(channelId: string, cls: "top" | "all", claim: MockClaim) {
    const room = this.state.coverage[channelId] ?? { top: [], all: [] };
    room[cls] = applyClaim(room[cls], claim);
    this.state.coverage[channelId] = room;
    this.save(this.state);
  }

  claimThread(rootId: string, claim: MockClaim) {
    this.state.threadCoverage[rootId] = applyClaim(
      this.state.threadCoverage[rootId] ?? [],
      claim,
    );
    this.save(this.state);
  }

  beginLive(wsId: number, channelIds: string[], since: number) {
    const id =
      this.state.segments.reduce(
        (max, segment) => Math.max(max, segment.id),
        0,
      ) + 1;
    this.state.segments.push({
      id,
      wsId,
      startedAt: since,
      closedAt: null,
      channelIds: [...new Set(channelIds)],
    });
    this.save(this.state);
    return id;
  }

  /** One open segment's rooms gain `all` [since, closedAt]; then it closes. */
  closeSegment(segmentId: number, closedAt: number) {
    const segment = this.state.segments.find(
      (candidate) => candidate.id === segmentId && candidate.closedAt === null,
    );
    if (!segment) return;
    const claim: MockClaim = {
      lo: { createdAt: segment.startedAt, eventId: TIME_FLOOR_ID },
      loIsStart: false,
      hi: { at: closedAt },
    };
    for (const channelId of segment.channelIds) {
      this.claimRoom(channelId, "all", claim);
    }
    segment.closedAt = Math.max(closedAt, segment.startedAt);
    this.save(this.state);
  }

  closeLive(wsId: number | null, closedAt: number) {
    for (const segment of this.state.segments) {
      if (segment.closedAt !== null) continue;
      if (wsId !== null && segment.wsId !== wsId) continue;
      this.closeSegment(segment.id, closedAt);
    }
    this.save(this.state);
  }

  /** Boot: segments left open close at the last frame written. */
  recoverOpenSegments() {
    if (this.state.lastFrameAt !== null) {
      this.closeLive(null, this.state.lastFrameAt);
      return;
    }
    for (const segment of this.state.segments) {
      segment.closedAt ??= segment.startedAt;
    }
    this.save(this.state);
  }

  private liveSince(channelId: string): number | null {
    const open = this.state.segments.filter(
      (segment) =>
        segment.closedAt === null && segment.channelIds.includes(channelId),
    );
    return open.length
      ? Math.min(...open.map((segment) => segment.startedAt))
      : null;
  }

  private topRuns(channelId: string) {
    const room = this.state.coverage[channelId] ?? { top: [], all: [] };
    const effective = normalize([...room.top, ...room.all]);
    return {
      effective,
      all: normalize(room.all),
      runs: withLive(effective, this.liveSince(channelId)),
    };
  }

  private timeline(channelId: string, run: Range) {
    return Object.values(this.state.events).filter(
      (row) =>
        row.channelId === channelId &&
        (row.role === 0 || row.role === 2) &&
        !row.deleted &&
        WINDOW_ROW_KINDS.has(row.kind) &&
        row.createdAt <= run.hiAt,
    );
  }

  private rowsBelow(
    channelId: string,
    run: Range,
    below: MessageStoreCursor | null,
    limit: number,
  ) {
    return this.timeline(channelId, run)
      .filter(
        (row) =>
          (!below ||
            cmpPos(row.createdAt, row.id, below.createdAt, below.eventId) <
              0) &&
          (run.loIsStart ||
            cmpPos(row.createdAt, row.id, run.loAt, run.loId) >= 0),
      )
      .sort((a, b) => b.createdAt - a.createdAt || cmpStr(a.id, b.id))
      .slice(0, limit);
  }

  private rowsFrom(
    channelId: string,
    run: Range,
    from: MessageStoreCursor,
    limit: number,
  ) {
    return this.timeline(channelId, run)
      .filter(
        (row) =>
          cmpPos(row.createdAt, row.id, from.createdAt, from.eventId) >= 0,
      )
      .sort((a, b) => a.createdAt - b.createdAt || cmpStr(b.id, a.id))
      .slice(0, limit);
  }

  private overlays(rows: StoredRow[]) {
    const auxIds = new Set(
      rows.flatMap((row) => this.state.targets[row.id] ?? []),
    );
    const aux = [...auxIds]
      .map((id) => this.state.events[id])
      .filter((row) => row && row.role === 3 && !row.deleted)
      .sort((a, b) => a.createdAt - b.createdAt || cmpStr(a.id, b.id))
      .map((row) => row.raw);
    const summaries = rows
      .map((row) => row.id)
      .sort(cmpStr)
      .flatMap((id) => {
        const summary = this.state.summaries[id];
        return summary ? [summary.payload] : [];
      });
    return { aux, summaries };
  }

  readWindow(
    channelId: string,
    before: MessageStoreCursor | null,
    limit: number,
  ): MessageStoreWindowPage {
    const { runs } = this.topRuns(channelId);
    const run = before
      ? runs.find((range) => contains(range, before.createdAt, before.eventId))
      : runs[runs.length - 1];
    let rows = run ? this.rowsBelow(channelId, run, before, limit + 1) : [];
    const full = rows.length > limit;
    rows = rows.slice(0, limit);
    const last = rows[rows.length - 1];
    const lastCursor = last
      ? { createdAt: last.createdAt, eventId: last.id }
      : before;
    const reachedStart = Boolean(run?.loIsStart) && !full;
    const nextCursor = reachedStart ? null : (lastCursor ?? null);
    return {
      startCursor: before,
      rows: rows.map((row) => row.raw),
      ...this.overlays(rows),
      nextCursor,
      hasMore: nextCursor !== null,
      gap: !full && !reachedStart,
    };
  }

  readAround(
    channelId: string,
    anchor: { createdAt: number; eventId?: string | null },
    before: number,
    after: number,
  ): MessageStoreAroundPage {
    const at = {
      createdAt: anchor.createdAt,
      eventId: anchor.eventId ?? TIME_FLOOR_ID,
    };
    const { runs } = this.topRuns(channelId);
    const run = runs.find((range) => contains(range, at.createdAt, at.eventId));
    if (!run) {
      return {
        rows: [],
        aux: [],
        summaries: [],
        hasOlder: true,
        hasNewer: true,
        gapBelow: true,
        gapAbove: true,
      };
    }
    const newer = this.rowsFrom(channelId, run, at, after + 1);
    const older = this.rowsBelow(channelId, run, at, before + 1);
    const newerFull = newer.length > after;
    const olderFull = older.length > before;
    const rows = [
      ...newer.slice(0, after).reverse(),
      ...older.slice(0, before),
    ];
    const gapBelow = !olderFull && !run.loIsStart;
    const gapAbove = !newerFull && run.hiAt !== LIVE_HI;
    return {
      rows: rows.map((row) => row.raw),
      ...this.overlays(rows),
      hasOlder: olderFull || gapBelow,
      hasNewer: newerFull || gapAbove,
      gapBelow,
      gapAbove,
    };
  }

  readThread(
    rootId: string,
    cursor: MessageStoreCursor | null,
    limit: number,
  ): MessageStoreThreadPage {
    const root = rootId.toLowerCase();
    const matching = Object.values(this.state.events)
      .filter(
        (row) =>
          row.rootId === root &&
          (row.role === 1 || row.role === 2) &&
          !row.deleted &&
          (!cursor ||
            row.createdAt > cursor.createdAt ||
            (row.createdAt === cursor.createdAt && row.id > cursor.eventId)),
      )
      .sort((a, b) => a.createdAt - b.createdAt || cmpStr(a.id, b.id));
    const hasMore = matching.length > limit;
    const rows = matching.slice(0, limit);
    const starts = normalize(this.state.threadCoverage[root] ?? []).filter(
      (range) => range.loIsStart,
    );
    let through: number | null = starts.length
      ? Math.max(...starts.map((range) => range.hiAt))
      : null;
    const rootRow = this.state.events[root];
    if (rootRow) {
      const { all } = this.topRuns(rootRow.channelId);
      const run = withLive(all, this.liveSince(rootRow.channelId)).find(
        (range) => contains(range, rootRow.createdAt, root),
      );
      if (run) through = Math.max(through ?? run.hiAt, run.hiAt);
    }
    const last = rows[rows.length - 1];
    return {
      rows: rows.map((row) => row.raw),
      ...this.overlays(rows),
      nextCursor:
        hasMore && last
          ? { createdAt: last.createdAt, eventId: last.id }
          : null,
      hasMore,
      coveredThrough: through === LIVE_HI ? null : through,
      liveCovered: through === LIVE_HI,
    };
  }

  /** Mirrors `read::thread_runs`: thread claims plus the root's `all` runs. */
  private threadRuns(root: string): Range[] {
    const ranges = [...normalize(this.state.threadCoverage[root] ?? [])];
    const rootRow = this.state.events[root];
    if (rootRow) {
      const { all } = this.topRuns(rootRow.channelId);
      for (const run of withLive(all, this.liveSince(rootRow.channelId))) {
        if (contains(run, rootRow.createdAt, root)) {
          ranges.push({ ...run, loAt: 0, loId: "", loIsStart: true });
        } else if (run.hiAt >= rootRow.createdAt) {
          ranges.push(run);
        }
      }
    }
    return normalize(ranges);
  }

  readThreadHead(
    rootId: string,
    before: MessageStoreCursor | null,
    limit: number,
  ): MessageStoreThreadHeadPage {
    const root = rootId.toLowerCase();
    const runs = this.threadRuns(root);
    const run = before
      ? runs.find((range) => contains(range, before.createdAt, before.eventId))
      : runs[runs.length - 1];
    let rows = run
      ? Object.values(this.state.events)
          .filter(
            (row) =>
              row.rootId === root &&
              (row.role === 1 || row.role === 2) &&
              !row.deleted &&
              row.createdAt <= run.hiAt &&
              (!before ||
                cmpPos(
                  row.createdAt,
                  row.id,
                  before.createdAt,
                  before.eventId,
                ) < 0) &&
              (run.loIsStart ||
                cmpPos(row.createdAt, row.id, run.loAt, run.loId) >= 0),
          )
          .sort((a, b) => b.createdAt - a.createdAt || cmpStr(a.id, b.id))
          .slice(0, limit + 1)
      : [];
    const full = rows.length > limit;
    rows = rows.slice(0, limit);
    const last = rows[rows.length - 1];
    const lastCursor = last
      ? { createdAt: last.createdAt, eventId: last.id }
      : before;
    const reachedStart = Boolean(run?.loIsStart) && !full;
    const nextCursor = reachedStart ? null : (lastCursor ?? null);
    const hiAt = run?.hiAt ?? null;
    return {
      rows: rows.map((row) => row.raw),
      ...this.overlays(rows),
      nextCursor,
      hasMore: nextCursor !== null,
      gap: !full && !reachedStart,
      coveredThrough: hiAt === LIVE_HI ? null : hiAt,
      liveCovered: hiAt === LIVE_HI,
    };
  }

  coverage(channelId: string): MessageStoreRoomCoverage {
    const { effective, all, runs } = this.topRuns(channelId);
    const view = (cls: "top" | "all", ranges: Range[]): MessageStoreRange[] =>
      ranges.map(({ loAt, loId, loIsStart, hiAt }) => ({
        class: cls,
        loAt,
        loId,
        loIsStart,
        hiAt,
      }));
    const since = this.liveSince(channelId);
    const newest = effective[effective.length - 1];
    const times = Object.values(this.state.events)
      .filter(
        (row) =>
          row.channelId === channelId && (row.role === 0 || row.role === 2),
      )
      .map((row) => row.createdAt);
    const lastRun = runs[runs.length - 1];
    return {
      channelId,
      ranges: [...view("top", effective), ...view("all", all)],
      liveCovered:
        since !== null && newest !== undefined && newest.hiAt >= since,
      provenThrough: all.length
        ? Math.max(...all.map((range) => range.hiAt))
        : null,
      newestLocalAt: times.length ? Math.max(...times) : null,
      gapBelow: !lastRun?.loIsStart,
    };
  }

  loadHeads(rooms: number, rows: number): MessageStoreRoomHead[] {
    return Object.entries(this.state.roomMeta)
      .sort(
        ([aId, a], [bId, b]) =>
          (b.lastOpenedAt ?? 0) - (a.lastOpenedAt ?? 0) ||
          b.lastEventAt - a.lastEventAt ||
          cmpStr(aId, bId),
      )
      .slice(0, rooms)
      .map(([channelId]) => ({
        channelId,
        page: this.readWindow(channelId, null, rows),
      }))
      .filter((head) => head.page.rows.length > 0);
  }
}

function scopeKey(scope: MessageStoreScope) {
  const origin = scope.relayUrl
    .trim()
    .replace(/\/+$/, "")
    .replace(/^wss:\/\//i, "https://")
    .replace(/^ws:\/\//i, "http://")
    .toLowerCase();
  return `kuzz-e2e-message-store:${scope.pubkey.trim().toLowerCase()}@${origin}`;
}

/** All scopes; `storage` null keeps everything in memory (node tests). */
export function createMockMessageStore(storage: ScopeStorage) {
  const open = new Map<string, MockScopeStore>();
  return {
    scope(scope: MessageStoreScope): MockScopeStore {
      if (!isHex64(scope.pubkey.trim())) {
        throw new Error("invalid message store scope");
      }
      const key = scopeKey(scope);
      const existing = open.get(key);
      if (existing) return existing;
      const saved = storage?.getItem(key);
      const store = new MockScopeStore(
        saved ? (JSON.parse(saved) as State) : emptyState(),
        (state) => storage?.setItem(key, JSON.stringify(state)),
      );
      store.recoverOpenSegments();
      open.set(key, store);
      return store;
    },
    clear(scope: MessageStoreScope) {
      const key = scopeKey(scope);
      open.delete(key);
      storage?.removeItem(key);
    },
  };
}

let bridgeStore: ReturnType<typeof createMockMessageStore> | null = null;

/** The bridge's (localStorage-backed) store for one scope. */
export function mockMessageStoreScope(scope: MessageStoreScope) {
  bridgeStore ??= createMockMessageStore(window.localStorage);
  return bridgeStore.scope(scope);
}

/** The `message_store_*` Tauri commands, for `e2eBridge.ts`. */
export function handleMessageStoreMockCommand(
  command: string,
  payload: unknown,
): unknown {
  bridgeStore ??= createMockMessageStore(window.localStorage);
  const args = (payload ?? {}) as Record<string, unknown> & {
    scope: MessageStoreScope;
  };
  const clamp = (value: unknown, fallback: number, max = 500) =>
    Math.min(Math.max(Number(value ?? fallback) || 1, 1), max);
  switch (command) {
    case "message_store_clear_scope":
      bridgeStore.clear(args.scope);
      return null;
    case "message_store_load_heads":
      return bridgeStore
        .scope(args.scope)
        .loadHeads(clamp(args.rooms, 64, 256), clamp(args.rows, 50));
    case "message_store_read_window":
      return bridgeStore
        .scope(args.scope)
        .readWindow(
          args.channelId as string,
          (args.before as MessageStoreCursor | null) ?? null,
          clamp(args.limit, 50),
        );
    case "message_store_read_around":
      return bridgeStore
        .scope(args.scope)
        .readAround(
          args.channelId as string,
          args.anchor as { createdAt: number; eventId?: string },
          clamp(args.before, 50),
          clamp(args.after, 50),
        );
    case "message_store_read_thread":
      return bridgeStore
        .scope(args.scope)
        .readThread(
          args.rootId as string,
          (args.cursor as MessageStoreCursor | null) ?? null,
          clamp(args.limit, 50),
        );
    case "message_store_read_thread_head":
      return bridgeStore
        .scope(args.scope)
        .readThreadHead(
          args.rootId as string,
          (args.before as MessageStoreCursor | null) ?? null,
          clamp(args.limit, 50),
        );
    case "message_store_coverage":
      return (args.channelIds as string[]).map((channelId) =>
        bridgeStore?.scope(args.scope).coverage(channelId),
      );
    case "message_store_begin_live":
      return bridgeStore
        .scope(args.scope)
        .beginLive(
          args.wsId as number,
          args.channelIds as string[],
          args.since as number,
        );
    default:
      throw new Error(`Unsupported message store command: ${command}`);
  }
}
