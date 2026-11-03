/**
 * Pure catch-up rules of the user stream: snapshot classes at the stream
 * EOSE, batching, the new-row rule and the gates built on it, the
 * mid-connection fallback plan, span stop, chunk handoff and membership.
 * Rules: docs/message-sync.md#user-stream; cases: test-fixtures/user-stream.json.
 */
import { USER_STREAM_LIMITS, type UserStreamMode } from "./userStreamRules";

const {
  lookbackSecs,
  catchUpMaxAgeSecs,
  catchUpBatchRooms,
  listenerFloorSecs,
  liveMissedAfterEoseSecs,
  liveMissedBeforeLastFrameSecs,
  spanExtendSecs,
  auditSampleMax,
} = USER_STREAM_LIMITS;

export type SnapshotClass = "uncovered" | "equal" | "behind" | "ahead";

export type SnapshotPlan = {
  classes: Map<string, SnapshotClass>;
  /** Equal rooms gated does not walk (only the stream's [L, L] is recorded). */
  skip: string[];
  /** Equal rooms live and shadow would have skipped (counted, verified). */
  predictedSkips: string[];
  /** Rooms the stream's catch-up walks (resume-ordered later by batching). */
  walk: string[];
  /** Ahead rooms: their `room_seq` row is deleted (and logged). */
  clearedRows: string[];
  /** Uncovered rooms: not fetched, opened as today. */
  notFetched: string[];
};

/**
 * Classes each snapshot room at the stream EOSE against its `room_seq` row
 * (`rows`: seq per room) and its coverage (`resume`: resume point per room).
 */
export function classifySnapshot(
  mode: UserStreamMode,
  snapshot: ReadonlyMap<string, number>,
  rows: ReadonlyMap<string, number>,
  resume: ReadonlyMap<string, number>,
): SnapshotPlan {
  const plan: SnapshotPlan = {
    classes: new Map(),
    skip: [],
    predictedSkips: [],
    walk: [],
    clearedRows: [],
    notFetched: [],
  };
  for (const [room, seq] of snapshot) {
    const stored = rows.get(room);
    let snapshotClass: SnapshotClass;
    if (!resume.has(room)) snapshotClass = "uncovered";
    else if (stored === undefined || stored < seq) snapshotClass = "behind";
    else if (stored === seq) snapshotClass = "equal";
    else snapshotClass = "ahead";
    plan.classes.set(room, snapshotClass);
    if (snapshotClass === "uncovered") {
      plan.notFetched.push(room);
      continue;
    }
    if (snapshotClass === "ahead") plan.clearedRows.push(room);
    if (snapshotClass === "equal") {
      if (mode === "gated") {
        plan.skip.push(room);
        continue;
      }
      plan.predictedSkips.push(room);
    }
    if (mode !== "shadow") plan.walk.push(room);
  }
  return plan;
}

/**
 * A room's coverage from before this connection (`connectionSpans`): only
 * segments that began before `connectedAt` count. `resume` is the walk's
 * resume point, `head` the newest such segment (the new-row base).
 */
export function coverageBefore(
  segments: readonly (readonly [number, number])[],
  connectedAt: number,
): { resume: number | null; head: [number, number] | null } {
  let resume: number | null = null;
  let head: [number, number] | null = null;
  for (const [lo, hi] of segments) {
    if (lo >= connectedAt) continue;
    if (resume === null || hi > resume) resume = hi;
    if (head === null || lo > head[0]) head = [lo, hi];
  }
  return { resume, head };
}

export type CatchUpBatch = { rooms: string[]; since: number };

/** A room whose own resume point is too old for a catch-up walk. */
export function isStaleResume(resumeAt: number, liveAt: number) {
  return resumeAt - lookbackSecs < liveAt - catchUpMaxAgeSecs;
}

/**
 * Stale rooms first (left for opening, recording nothing), then the rest
 * sorted by resume point and id, `catchUpBatchRooms` per batch.
 */
export function planCatchUpBatches(
  resume: ReadonlyMap<string, number> | Iterable<[string, number]>,
  liveAt: number,
): { stale: string[]; batches: CatchUpBatch[] } {
  const stale: string[] = [];
  const fresh: [string, number][] = [];
  for (const [room, at] of resume) {
    if (isStaleResume(at, liveAt)) stale.push(room);
    else fresh.push([room, at]);
  }
  stale.sort();
  fresh.sort(([a, atA], [b, atB]) => atA - atB || (a < b ? -1 : a > b ? 1 : 0));
  const batches: CatchUpBatch[] = [];
  for (let start = 0; start < fresh.length; start += catchUpBatchRooms) {
    const slice = fresh.slice(start, start + catchUpBatchRooms);
    batches.push({
      rooms: slice.map(([room]) => room),
      since: slice[0][1] - lookbackSecs,
    });
  }
  return { stale, batches };
}

export type NewRowInput = {
  liveAt: number;
  /** The room's coverage segments `[lo, hi]` before the walk recorded. */
  segments: readonly (readonly [number, number])[];
  inserted: boolean;
  createdAt: number;
  /** The id was also received live on this connection (stream or chunk). */
  receivedLive?: boolean;
};

/**
 * A walked row is new when the walk inserted it, it was not received live
 * on this connection, and it lies in `[base, L]`, base being the start of
 * the newest segment that starts before L (never the stream's own [L, L]).
 */
export function isNewRow(input: NewRowInput) {
  if (!input.inserted || input.receivedLive) return false;
  if (input.createdAt > input.liveAt) return false;
  let base: number | null = null;
  for (const [lo] of input.segments) {
    if (lo < input.liveAt && (base === null || lo > base)) base = lo;
  }
  return base !== null && input.createdAt >= base;
}

/** Live mode: a predicted skip with a completed walk and no new row. */
export function verifySkips(
  rooms: readonly { walked: boolean; newRows: number }[],
) {
  let verifiedSkips = 0;
  let falseSkip = 0;
  for (const room of rooms) {
    if (!room.walked) continue;
    if (room.newRows === 0) verifiedSkips += 1;
    else falseSkip += 1;
  }
  return { verifiedSkips, falseSkip };
}

/** Gated: skipped rooms whose resume point is inside the walk age limit. */
export function auditEligible(
  skipped: ReadonlyMap<string, number> | Iterable<[string, number]>,
  liveAt: number,
) {
  const eligible: string[] = [];
  for (const [room, at] of skipped) {
    if (!isStaleResume(at, liveAt)) eligible.push(room);
  }
  return eligible.sort();
}

/** A uniform sample of at most `auditSampleMax` rooms (partial shuffle). */
export function auditSample(
  eligible: readonly string[],
  random: () => number = Math.random,
) {
  const pool = [...eligible];
  const size = Math.min(auditSampleMax, pool.length);
  for (let index = 0; index < size; index += 1) {
    const pick = index + Math.floor(random() * (pool.length - index));
    [pool[index], pool[pick]] = [pool[pick], pool[index]];
  }
  return pool.slice(0, size);
}

export function auditVerdicts(
  batches: readonly {
    rooms: readonly string[];
    completed: boolean;
    newRows: Readonly<Record<string, number>>;
  }[],
) {
  let skipAudited = 0;
  let skipAuditNew = 0;
  let skipAuditFailed = 0;
  for (const batch of batches) {
    for (const room of batch.rooms) {
      if (!batch.completed) {
        skipAuditFailed += 1;
        continue;
      }
      skipAudited += 1;
      if ((batch.newRows[room] ?? 0) > 0) skipAuditNew += 1;
    }
  }
  return { skipAudited, skipAuditNew, skipAuditFailed };
}

export type PreviousLive = {
  liveRooms: readonly string[];
  eoseAt: number | null;
  lastFrameAt: number | null;
};

/** The inclusive liveMissed window of the previous live connection. */
export function liveMissedWindow(prev: PreviousLive) {
  if (prev.eoseAt === null || prev.lastFrameAt === null) return null;
  const from = prev.eoseAt + liveMissedAfterEoseSecs;
  const to = prev.lastFrameAt - liveMissedBeforeLastFrameSecs;
  return from <= to ? { from, to } : null;
}

export type WalkedRow = NewRowInput & {
  room: string;
  kind: number;
  pubkey: string;
};

/**
 * Rows the stream should have delivered on the previous live connection:
 * new rows of its live rooms whose batch completed, inside the window,
 * minus the relay's own 9005s.
 */
export function liveMissed({
  prev,
  walked,
  rows,
  relayPubkey,
}: {
  prev: PreviousLive;
  walked: ReadonlyMap<string, boolean>;
  rows: readonly WalkedRow[];
  relayPubkey: string;
}) {
  const checked = new Set(prev.liveRooms.filter((room) => walked.get(room)));
  const window = liveMissedWindow(prev);
  let missed = 0;
  if (window) {
    for (const row of rows) {
      if (!checked.has(row.room)) continue;
      if (row.kind === 9005 && row.pubkey === relayPubkey) continue;
      if (row.createdAt < window.from || row.createdAt > window.to) continue;
      if (isNewRow(row)) missed += 1;
    }
  }
  return { liveRoomsChecked: checked.size, liveMissed: missed };
}

/** Rooms left on chunks: after an accepted live/gated EOSE, the snapshot's leave. */
export function chunkRoomsAfterHandoff({
  mode,
  streamLive,
  rooms,
  snapshotRooms,
}: {
  mode: UserStreamMode | null;
  streamLive: boolean;
  rooms: Iterable<string>;
  snapshotRooms: ReadonlySet<string>;
}) {
  const list = [...rooms];
  if (!streamLive || (mode !== "live" && mode !== "gated")) return list;
  return list.filter((room) => !snapshotRooms.has(room));
}

export type StreamState =
  | "pending"
  | "none"
  | "shadow"
  | "liveEose"
  | "gatedEose"
  | "closedAfterEose";

/** Rooms whose chunk catch-up and chunk repair run (`chunkCatchUp`). */
export function chunkCatchUpRooms(
  state: StreamState,
  chunkRooms: Iterable<string>,
  snapshotRooms: ReadonlySet<string>,
) {
  const rooms = [...chunkRooms];
  if (state !== "liveEose" && state !== "gatedEose") return rooms;
  return rooms.filter((room) => !snapshotRooms.has(room));
}

export type MidConnectionPlan = {
  chunkRooms: string[];
  catchUp: { stale: string[]; batches: CatchUpBatch[] } | null;
  desktopRepair: {
    rooms: string[];
    since: number;
    listenerFloor: Record<string, number>;
  } | null;
  awaitSocketCloseSecs?: number;
};

/** What a stream CLOSED on this connection starts (`midConnectionClosed`). */
export function midConnectionPlan({
  mode,
  streamLive,
  awaitSocketClose,
  closedAt,
  carriedRooms,
  chunkExtras,
  resume,
  lastSeenCreatedAt,
  liveSince,
}: {
  mode: UserStreamMode | null;
  streamLive: boolean;
  awaitSocketClose: boolean;
  closedAt: number;
  carriedRooms: readonly string[];
  chunkExtras: readonly string[];
  resume: ReadonlyMap<string, number>;
  lastSeenCreatedAt: number | null;
  liveSince: ReadonlyMap<string, number>;
}): MidConnectionPlan {
  if (awaitSocketClose && streamLive && mode !== "shadow") {
    return {
      chunkRooms: [],
      catchUp: null,
      desktopRepair: null,
      awaitSocketCloseSecs: USER_STREAM_LIMITS.socketCloseGraceMs / 1000,
    };
  }
  const chunkRooms = [...new Set([...carriedRooms, ...chunkExtras])];
  if (!streamLive || (mode !== "live" && mode !== "gated")) {
    return { chunkRooms, catchUp: null, desktopRepair: null };
  }
  const covered = carriedRooms
    .filter((room) => resume.has(room))
    .map((room) => [room, resume.get(room) as number] as [string, number]);
  const listenerFloor: Record<string, number> = {};
  for (const room of carriedRooms) {
    const since = liveSince.get(room);
    if (since !== undefined) listenerFloor[room] = since - listenerFloorSecs;
  }
  return {
    chunkRooms,
    catchUp: planCatchUpBatches(covered, closedAt),
    desktopRepair:
      lastSeenCreatedAt === null
        ? null
        : {
            rooms: [...carriedRooms],
            since: lastSeenCreatedAt - lookbackSecs,
            listenerFloor,
          },
  };
}

/**
 * The stream's coverage span on one connection: `[L, L]` at its EOSE,
 * extended by stream frames at most once per `spanExtendSecs`, frozen at
 * its CLOSED (`spanStop`).
 */
export class StreamSpan {
  private from: number | null = null;
  private to = 0;
  private stopped = false;

  eose(at: number) {
    this.from = at;
    this.to = at;
    this.stopped = false;
  }

  /** A stream frame at `at`; returns the new end when the span grew. */
  frame(at: number) {
    if (this.from === null || this.stopped) return null;
    if (at - this.to < spanExtendSecs) return null;
    this.to = at;
    return at;
  }

  closed() {
    this.stopped = true;
  }

  get live() {
    return this.from !== null && !this.stopped;
  }

  get span(): [number, number] | null {
    return this.from === null ? null : [this.from, this.to];
  }
}

/**
 * Carried rooms and the 44101 render rule in live/gated after the stream
 * EOSE (`membership`).
 */
export class StreamMembership {
  readonly carried: Set<string>;
  private removedAt = new Map<string, number>();

  constructor(carried: Iterable<string>) {
    this.carried = new Set(carried);
  }

  /** 44101: the room stops being carried. */
  removed(room: string, createdAt: number) {
    this.removedAt.set(room, createdAt);
    this.carried.delete(room);
  }

  /**
   * 44100: returns the room's coverage span start (its receive time) when
   * the stream starts carrying it, else null.
   */
  joined(room: string, createdAt: number, receivedAt: number) {
    const removed = this.removedAt.get(room);
    if (removed !== undefined && createdAt < removed) return null;
    this.removedAt.delete(room);
    if (this.carried.has(room)) return null;
    this.carried.add(room);
    return receivedAt;
  }

  /** Whether a stream frame is shown (it is always stored). */
  renders(room: string, createdAt: number) {
    const removed = this.removedAt.get(room);
    return removed === undefined || createdAt <= removed;
  }
}

export type StreamRoute = {
  store: boolean;
  openRoom: boolean;
  allRooms: boolean;
  listRefresh: boolean;
};

/** Where a live/gated stream frame goes (`routing`). */
export function routeStreamFrame(
  room: string,
  lists: {
    memberRooms: ReadonlySet<string>;
    archivedRooms: ReadonlySet<string>;
    openRooms: ReadonlySet<string>;
  },
): StreamRoute {
  if (lists.memberRooms.has(room)) {
    return { store: true, openRoom: true, allRooms: true, listRefresh: false };
  }
  if (lists.archivedRooms.has(room)) {
    return {
      store: true,
      openRoom: lists.openRooms.has(room),
      allRooms: false,
      listRefresh: false,
    };
  }
  return { store: true, openRoom: false, allRooms: false, listRefresh: true };
}
