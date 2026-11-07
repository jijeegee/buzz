/**
 * The user stream in `live`/`gated` (docs/message-sync.md#user-stream):
 * handoff after the stream EOSE, routing of its frames, the batched
 * catch-up with rule-W watermark writes, the gated skip and its audit,
 * `liveMissed`, the listener repair, the mid-connection fallback and the
 * 44100/44101 hooks; in `shadow`, the skip prediction and its verification
 * through chunk catch-ups. The rules themselves are in `userStreamCatchUp.ts`.
 */
import type { ChannelReconnectRepairRequest } from "@/shared/api/channelReconnectRepair";
import {
  isRateLimited,
  waitForRateLimit,
} from "@/shared/api/relayRateLimitGate";
import { noteLiveRoomActivity } from "@/shared/api/liveRoomActivity";
import {
  endStreamCoverage,
  forgetStoreWindows,
  noteStreamLiveEose,
  STORE_CATCH_UP_MAX_PAGES,
  STORE_CATCH_UP_PAGE_LIMIT,
  setStreamCatchUpVouch,
} from "@/shared/api/relayStoreCoverage";
import type {
  UserStreamConnection,
  UserStreamLiveHooks,
  UserStreamReport,
} from "@/shared/api/relayUserStream";
import type { RelayEvent } from "@/shared/api/types";
import {
  auditEligible,
  auditSample,
  auditVerdicts,
  type CatchUpBatch,
  classifySnapshot,
  isNewRow,
  isStaleResume,
  liveMissed,
  midConnectionPlan,
  type PreviousLive,
  planCatchUpBatches,
  StreamMembership,
  StreamSpan,
  verifySkips,
  type WalkedRow,
} from "@/shared/api/userStreamCatchUp";
import {
  type ClosedDecision,
  USER_STREAM_LIMITS,
  type UserStreamSnapshot,
} from "@/shared/api/userStreamRules";

const KIND_MEMBER_ADDED = 44100;
const KIND_MEMBER_REMOVED = 44101;
/** Rows older than a room's live start minus this never reach listeners. */
const LISTENER_FLOOR_SKEW_SECS = 5;
const PREV_LIVE_KEY = "stream_prev_live";
/** Ids received live per connection; past this nothing is counted. */
const LIVE_IDS_MAX = 200_000;
const REPAIR_BATCH_ROOMS = 128;
/** Unknown-room list refresh: per room, and over all rooms. */
const UNKNOWN_ROOM_INTERVAL_MS = 10_000;
const UNKNOWN_ROOM_MIN_GAP_MS = 5_000;

export type StoredSeqs = {
  /** Everything queued was written and nothing was lost when read. */
  vouched: boolean;
  epoch: string | null;
  rows: Record<string, [number, number]>;
  /** Resume points from coverage begun before the asked time. */
  resume: Record<string, number>;
};

const NO_SEQS: StoredSeqs = {
  vouched: false,
  epoch: null,
  rows: {},
  resume: {},
};

/** The device store as the stream uses it (`message_store_*` commands). */
export type StreamStore = {
  /** Queues an accepted epoch ahead of this connection's rule-W writes. */
  epoch: (epoch: string) => Promise<boolean>;
  /** One drained read: epoch, rows and resume points before `before`. */
  seqs: (roomIds: string[], before?: number) => Promise<StoredSeqs>;
  clearRows: (roomIds: string[]) => Promise<void>;
  scope: (subId?: string) => Promise<string | null>;
  resumePoints: (
    roomIds: string[],
    before: number,
  ) => Promise<Record<string, number>>;
  /** Each room's coverage segments before a walk (`null`: not vouched). */
  segments: (
    roomIds: string[],
  ) => Promise<Record<string, [number, number][]> | null>;
  /** Which of `ids` were not stored before `sinceMs` (`null`: not vouched). */
  freshIds: (ids: string[], sinceMs: number) => Promise<string[] | null>;
  /** A catch-up span, with rule-W rows when `seq` is given. */
  extend: (
    roomIds: string[],
    from: number,
    to: number,
    scope: string,
    seq?: { epoch: string; seqs: Record<string, number> },
  ) => Promise<void>;
  getState: (key: string) => Promise<string | null>;
  setState: (key: string, value: string) => Promise<void>;
};

/** The room bundle as the stream hands rooms to it and from it. */
export type StreamBundle = {
  /**
   * `live: false`: off chunks, but not live (a stopped stream's wait). A
   * chunk carrying a room outside `rooms` stays as it is.
   */
  setStreamRooms: (rooms: ReadonlySet<string> | null, live?: boolean) => void;
  streamRoomsChanged: () => void;
  /** Delivers once by id; `false` when the room is unknown here. */
  dispatchStream: (event: RelayEvent, roomId: string) => boolean;
  roomLists: () => { carried: ReadonlySet<string>; extras: string[] };
};

export type UserStreamLiveHost = {
  nowSecs: () => number;
  store: StreamStore;
  bundle: StreamBundle;
  roomOf: (event: RelayEvent) => string | null;
  requestPage: (
    request: ChannelReconnectRepairRequest,
  ) => Promise<RelayEvent[]>;
  /** The session's counters (replaced on an account or community switch). */
  report: () => UserStreamReport;
  /** One (debounced) member list refresh. */
  listRefresh: () => void;
  log: (line: string) => void;
  random?: () => number;
  /** The room on screen: its listener repair goes first. */
  visibleRoom?: () => string | null;
  /** The relay's rate-limit gate (`relayRateLimitGate.ts`). */
  rateLimited?: () => boolean;
  waitForRateLimit?: () => Promise<void>;
  /** Clock and timer of the unknown-room refresh throttle. */
  nowMs?: () => number;
  setTimer?: (ms: number, run: () => void) => void;
};

/**
 * Frames of rooms the member list lacks: a list refresh at most once per
 * 10 s per room and at most one every 5 s over all rooms (a later one
 * waits out the gap). The listener debounces each burst.
 */
export class UnknownRoomRefresh {
  private readonly asked = new Map<string, number>();
  private last: number | null = null;
  private pending = false;

  private readonly refresh: () => void;
  private readonly nowMs: () => number;
  private readonly setTimer: (ms: number, run: () => void) => void;

  constructor(
    refresh: () => void,
    nowMs: () => number = () => Date.now(),
    setTimer: (ms: number, run: () => void) => void = (ms, run) => {
      setTimeout(run, ms);
    },
  ) {
    this.refresh = refresh;
    this.nowMs = nowMs;
    this.setTimer = setTimer;
  }

  note(room: string) {
    const now = this.nowMs();
    const asked = this.asked.get(room);
    if (asked !== undefined && now - asked < UNKNOWN_ROOM_INTERVAL_MS) return;
    for (const [key, at] of this.asked) {
      if (now - at >= UNKNOWN_ROOM_INTERVAL_MS) this.asked.delete(key);
    }
    this.asked.set(room, now);
    if (this.pending) return;
    const wait =
      this.last === null ? 0 : this.last + UNKNOWN_ROOM_MIN_GAP_MS - now;
    if (wait <= 0) {
      this.fire();
      return;
    }
    this.pending = true;
    this.setTimer(wait, () => {
      this.pending = false;
      this.fire();
    });
  }

  private fire() {
    this.last = this.nowMs();
    this.refresh();
  }
}

type Walk = { completed: boolean; events: RelayEvent[] };
type Counted = {
  /** New rows could be told (segments, stored-at and live ids known). */
  counted: boolean;
  newRows: Record<string, number>;
  rows: WalkedRow[];
};
const UNCOUNTED: Counted = { counted: false, newRows: {}, rows: [] };

/** Per room, the stream's `h` tag room of a fetched row. */
function roomTag(event: RelayEvent) {
  return event.tags.find((tag) => tag[0] === "h")?.[1] ?? null;
}

function pick<T>(record: Readonly<Record<string, T>>, keys: Iterable<string>) {
  const picked: Record<string, T> = {};
  for (const key of keys) {
    if (record[key] !== undefined) picked[key] = record[key];
  }
  return picked;
}

/** Rows of another epoch are none (its clear may not be written yet). */
function rowsOf(stored: StoredSeqs, epoch: string) {
  const rows = new Map<string, number>();
  if (stored.vouched && stored.epoch === epoch) {
    for (const [room, [seq]] of Object.entries(stored.rows)) {
      rows.set(room, seq);
    }
  }
  return rows;
}

/** The stream's state on one live/gated connection, after its EOSE. */
type LiveConnection = {
  stream: UserStreamConnection;
  snapshot: UserStreamSnapshot;
  liveAt: number;
  /** Spans that begin at or after this are this connection's own. */
  before: number;
  /** The counters at the EOSE; another account's session gets new ones. */
  report: UserStreamReport;
  scope: Promise<string | null>;
  membership: StreamMembership;
  /** Rooms off chunks: covered, fresh snapshot rooms, plus 44100 joins. */
  handed: Set<string>;
  /** Snapshot rooms with no usable resume point: they keep their chunk. */
  kept: Set<string>;
  span: StreamSpan;
  coverage: ReturnType<typeof noteStreamLiveEose>;
  /** The stream stopped (CLOSED or socket end): its walks start no page. */
  ended: boolean;
  /** The socket ended: the fallback's walks start no page either. */
  socketGone: boolean;
};

/** A shadow connection after its EOSE. */
type ShadowConnection = {
  stream: UserStreamConnection;
  report: UserStreamReport;
};

export class UserStreamLive implements UserStreamLiveHooks {
  private current: LiveConnection | null = null;
  private shadow: ShadowConnection | null = null;
  /** Newest stream frame `created_at` or EOSE time (in memory, as for chunks). */
  private lastSeenCreatedAt: number | null = null;
  /** When the stream first carried each room in this app run. */
  private readonly liveSince = new Map<string, number>();
  /**
   * Ids received live on this connection (stream or chunk frames), only
   * while its stream REQ is pending or accepted (`null`: none collected).
   */
  private liveIds: Set<string> | null = null;
  private readonly host: UserStreamLiveHost;
  private readonly unknownRooms: UnknownRoomRefresh;

  constructor(host: UserStreamLiveHost) {
    this.host = host;
    this.unknownRooms = new UnknownRoomRefresh(
      () => host.listRefresh(),
      host.nowMs,
      host.setTimer,
    );
  }

  /** The catch-up started at the last EOSE (tests). */
  settled: Promise<void> = Promise.resolve();
  /** The last walk phase queued (catch-up or fallback). */
  private walkTail: Promise<void> = Promise.resolve();

  /** Waits for every earlier walk phase; call the result when done. */
  private takeWalks() {
    const prior = this.walkTail;
    let release: () => void = () => {};
    this.walkTail = new Promise<void>((resolve) => {
      release = resolve;
    });
    return prior.then(() => release);
  }

  eose(stream: UserStreamConnection, snapshot: UserStreamSnapshot) {
    const { host } = this;
    const liveAt = host.nowSecs();
    // Queued before any rule-W write of this connection.
    const epochQueued = host.store.epoch(snapshot.epoch).catch(() => false);
    const scope = host.store.scope(stream.subId).catch(() => null);
    const membership = new StreamMembership(snapshot.rooms.keys());
    const handed = new Set<string>();
    const span = new StreamSpan();
    span.eose(liveAt);
    const coverage = noteStreamLiveEose({
      generation: stream.generation,
      liveAt,
      rooms: handed,
      scope,
    });
    const report = host.report();
    const live: LiveConnection = {
      stream,
      snapshot,
      liveAt,
      before: Math.min(stream.startedAt, liveAt),
      report,
      scope,
      membership,
      handed,
      kept: new Set(),
      span,
      coverage,
      ended: false,
      socketGone: false,
    };
    this.current = live;
    report.add("snapshotRooms", snapshot.rooms.size);
    const repairSince =
      this.lastSeenCreatedAt === null
        ? null
        : Math.max(0, this.lastSeenCreatedAt - USER_STREAM_LIMITS.lookbackSecs);
    // As a chunk cursor starts at its creation: the next connection repairs
    // from here even when no frame comes (`B1`).
    this.lastSeenCreatedAt = Math.max(this.lastSeenCreatedAt ?? liveAt, liveAt);
    this.settled = this.catchUp(live, epochQueued, repairSince).catch(
      (error: unknown) => host.log(`[user-stream] catch-up failed ${error}`),
    );
    coverage.settle(this.settled);
  }

  /** Shadow: the skip prediction, verified by later chunk catch-ups. */
  shadowEose(stream: UserStreamConnection, snapshot: UserStreamSnapshot) {
    const shadow: ShadowConnection = { stream, report: this.host.report() };
    this.shadow = shadow;
    this.settled = this.shadowCatchUp(shadow, snapshot).catch(
      (error: unknown) =>
        this.host.log(`[user-stream] shadow check failed ${error}`),
    );
  }

  /** The stream REQ is pending (`true`), or was refused or not sent. */
  collectLiveIds(on: boolean) {
    if (!on) this.liveIds = null;
    else this.liveIds ??= new Set();
  }

  noteLive(eventId: string) {
    const ids = this.liveIds;
    if (!ids) return;
    if (ids.size >= LIVE_IDS_MAX) {
      // Too many to tell: this connection's walks count nothing.
      this.liveIds = null;
      return;
    }
    ids.add(eventId);
  }

  scopeChanged() {
    this.lastSeenCreatedAt = null;
    this.liveSince.clear();
  }

  frame(stream: UserStreamConnection, event: RelayEvent) {
    const live = this.current;
    if (!live || live.stream !== stream || live.ended) return;
    const { host } = this;
    if (
      this.lastSeenCreatedAt === null ||
      event.created_at > this.lastSeenCreatedAt
    ) {
      this.lastSeenCreatedAt = event.created_at;
    }
    const grown = live.span.frame(stream.lastFrameAt ?? event.created_at);
    if (grown !== null) {
      live.coverage.extend(grown);
      void this.savePrevLive(live);
    }
    const room = host.roomOf(event);
    if (!room) {
      // Normal for a reaction on a row this client never saw.
      live.report.add("streamNoRoom");
      return;
    }
    noteLiveRoomActivity(room);
    // Always stored (by the store layer); shown unless left (44101).
    if (!live.membership.renders(room, event.created_at)) return;
    if (host.bundle.dispatchStream(event, room)) return;
    // Archived or hidden rooms stay unknown: throttled refreshes.
    this.unknownRooms.note(room);
  }

  ended(stream: UserStreamConnection, decision: ClosedDecision | null) {
    const live = this.current;
    if (!live || live.stream !== stream || live.ended) return;
    live.ended = true;
    live.span.closed();
    endStreamCoverage();
    if (decision?.action !== "fallback") {
      // Waiting for the socket to close: no chunk starts on the dying
      // socket, but its rooms no longer count as live (`socketEnded` puts
      // them on chunks).
      this.host.bundle.setStreamRooms(live.handed, false);
      return;
    }
    live.report.add(`fallback_${decision.key}`);
    void this.fallback(live).catch((error: unknown) =>
      this.host.log(`[user-stream] fallback failed ${error}`),
    );
  }

  /**
   * The socket ended: every room is on chunks again from the next connect.
   * `startedAt`: when that connection began, if it reached AUTH OK; its
   * chunks carried rooms from then, so the next live connection's listener
   * repair starts no later than that (`B1`, as a chunk cursor).
   */
  socketEnded(startedAt: number | null = null) {
    const { carried } = this.host.bundle.roomLists();
    if (startedAt !== null && carried.size > 0) {
      this.lastSeenCreatedAt = Math.max(
        this.lastSeenCreatedAt ?? startedAt,
        startedAt,
      );
      // Their listeners heard the chunks from then on.
      for (const room of carried) {
        if (!this.liveSince.has(room)) this.liveSince.set(room, startedAt);
      }
    }
    const live = this.current;
    this.current = null;
    if (live) {
      live.socketGone = true;
      if (!live.ended) {
        live.ended = true;
        live.span.closed();
        endStreamCoverage();
      }
    }
    if (this.shadow) {
      this.shadow = null;
      setStreamCatchUpVouch(null);
    }
    this.liveIds = null;
    this.host.bundle.setStreamRooms(null);
  }

  /** A live 44100/44101 for the account (`membership`). */
  membershipNotice(event: RelayEvent) {
    const live = this.current;
    if (!live || live.ended) return;
    const room = roomTag(event);
    if (!room) return;
    if (event.kind === KIND_MEMBER_REMOVED) {
      live.membership.removed(room, event.created_at);
      live.handed.delete(room);
      live.kept.delete(room);
      this.host.bundle.streamRoomsChanged();
    } else if (event.kind === KIND_MEMBER_ADDED) {
      const from = live.membership.joined(
        room,
        event.created_at,
        this.host.nowSecs(),
      );
      if (from === null) return;
      if (!this.liveSince.has(room)) this.liveSince.set(room, from);
      live.kept.delete(room);
      live.coverage.carry(room, from);
      live.handed.add(room);
      this.host.bundle.streamRoomsChanged();
    }
  }

  /**
   * Hands the snapshot rooms with a usable resume point to the stream; the
   * rest (uncovered, stale, or none vouched after a write loss) keep their
   * chunks and those chunks' reconnect repair (docs/message-sync.md#handoff).
   */
  private hand(live: LiveConnection, resume: ReadonlyMap<string, number>) {
    const handed: string[] = [];
    for (const room of live.membership.carried) {
      if (live.handed.has(room) || !live.snapshot.rooms.has(room)) continue;
      const at = resume.get(room);
      if (at === undefined || isStaleResume(at, live.liveAt)) {
        live.kept.add(room);
        continue;
      }
      handed.push(room);
      live.handed.add(room);
      if (!this.liveSince.has(room)) this.liveSince.set(room, live.liveAt);
    }
    live.coverage.hand(handed);
    live.report.add("handoffKept", live.kept.size);
    this.host.bundle.setStreamRooms(live.handed, true);
  }

  private async catchUp(
    live: LiveConnection,
    epochQueued: Promise<boolean>,
    repairSince: number | null,
  ) {
    const { host } = this;
    const { snapshot, liveAt, report } = live;
    const prev = await this.loadPrevLive();
    await this.savePrevLive(live);
    await epochQueued;
    if (live.ended) return;
    const rooms = [...snapshot.rooms.keys()];
    // Coverage from before this connection only: a chunk EOSE span `[C, C]`
    // recorded on it would hide an offline gap from the walk and the rows.
    const stored = await host.store
      .seqs(rooms, live.before)
      .catch(() => NO_SEQS);
    if (live.ended) return;
    const resume = new Map(stored.vouched ? Object.entries(stored.resume) : []);
    this.hand(live, resume);
    const plan = classifySnapshot(
      snapshot.mode,
      snapshot.rooms,
      rowsOf(stored, snapshot.epoch),
      resume,
    );
    // A gated skip is a completed catch-up: its stream span now.
    live.coverage.span(plan.skip);
    report.add("predictedSkips", plan.predictedSkips.length);
    if (plan.clearedRows.length > 0) {
      await host.store.clearRows(plan.clearedRows).catch(() => {});
      host.log(
        `[user-stream] stored seq ahead of the relay for ${plan.clearedRows.length} rooms; rows cleared`,
      );
    }
    const walkResume = new Map(
      plan.walk.flatMap((room) => {
        const at = resume.get(room);
        return at === undefined ? [] : [[room, at] as [string, number]];
      }),
    );
    const { stale, batches } = planCatchUpBatches(walkResume, liveAt);
    // Too old to walk: their windows are fetched when opened.
    forgetStoreWindows(stale);
    const sampled = this.auditPick(live, plan.skip, resume);
    // Walks start after every earlier connection's walks ended (`S5`).
    const release = await this.takeWalks();
    try {
      const stop = () => live.ended;
      // The listener repair does not wait for the catch-up (`S4`).
      const repairing =
        repairSince === null
          ? Promise.resolve()
          : this.repair(
              live,
              [...batches.flatMap((batch) => batch.rooms), ...sampled.keys()],
              repairSince,
              {},
              stop,
            );
      const walked = new Map<string, boolean>();
      const newRows: Record<string, number> = {};
      const walkedRows: WalkedRow[] = [];
      for (const batch of batches) {
        if (stop()) break;
        const result = await this.walkBatch(live, batch, true, true, stop);
        // Only a completed batch gets the stream span: a cut-short one
        // resumes from its earlier coverage on the next connection.
        if (result.completed) live.coverage.span(batch.rooms);
        else if (!stop()) forgetStoreWindows(batch.rooms);
        if (result.completed && !result.counted) {
          report.add("newRowUncounted", batch.rooms.length);
        }
        for (const room of batch.rooms) walked.set(room, result.counted);
        Object.assign(newRows, result.newRows);
        walkedRows.push(...result.rows);
      }
      if (snapshot.mode === "live") {
        const verdict = verifySkips(
          plan.predictedSkips.map((room) => ({
            walked: walked.get(room) === true,
            newRows: newRows[room] ?? 0,
          })),
        );
        report.add("verifiedSkips", verdict.verifiedSkips);
        report.add("falseSkip", verdict.falseSkip);
      }
      await this.audit(live, sampled, stop);
      if (prev && !stop()) {
        const missed = liveMissed({
          prev,
          walked,
          rows: walkedRows,
          relayPubkey: snapshot.relayPubkey,
        });
        report.add("liveRoomsChecked", missed.liveRoomsChecked);
        report.add("liveMissed", missed.liveMissed);
      }
      await repairing;
    } finally {
      release();
    }
  }

  /** Gated: the skipped rooms sampled for the audit (`auditSampling`). */
  private auditPick(
    live: LiveConnection,
    skipped: readonly string[],
    resume: ReadonlyMap<string, number>,
  ) {
    if (live.snapshot.mode !== "gated" || skipped.length === 0) {
      return new Map<string, number>();
    }
    const skippedResume = new Map(
      skipped.flatMap((room) => {
        const at = resume.get(room);
        return at === undefined ? [] : [[room, at] as [string, number]];
      }),
    );
    const eligible = auditEligible(skippedResume, live.liveAt);
    const sample = auditSample(eligible, this.host.random);
    return new Map(sample.map((room) => [room, skippedResume.get(room) ?? 0]));
  }

  /** Gated: walks the sampled skipped rooms (`auditVerdicts`). */
  private async audit(
    live: LiveConnection,
    sampled: ReadonlyMap<string, number>,
    stop: () => boolean,
  ) {
    if (sampled.size === 0) return;
    const results = [];
    for (const batch of planCatchUpBatches(sampled, live.liveAt).batches) {
      if (stop()) return;
      const result = await this.walkBatch(live, batch, true, true, stop);
      results.push({
        rooms: batch.rooms,
        // A walk whose new rows cannot be told proves nothing either way.
        completed: result.counted,
        newRows: result.newRows,
      });
    }
    const verdict = auditVerdicts(results);
    const { report } = live;
    report.add("skipAudited", verdict.skipAudited);
    report.add("skipAuditNew", verdict.skipAuditNew);
    report.add("skipAuditFailed", verdict.skipAuditFailed);
  }

  /**
   * One catch-up batch up to the stream EOSE (L). A completed walk records
   * its span, with rule-W rows when `withSeq` (rule W: it started after
   * this connection's stream EOSE), and, when `count`, counts its new rows.
   */
  private async walkBatch(
    live: LiveConnection,
    batch: CatchUpBatch,
    withSeq: boolean,
    count: boolean,
    stop: () => boolean,
  ): Promise<Counted & { completed: boolean }> {
    const { host } = this;
    const { liveAt, snapshot } = live;
    const segments = count
      ? await host.store.segments(batch.rooms).catch(() => null)
      : null;
    const walk = await this.walk(batch.rooms, batch.since, stop);
    if (!walk.completed) return { completed: false, ...UNCOUNTED };
    const scope = await live.scope;
    if (scope !== null) {
      const seqs = pick(Object.fromEntries(snapshot.rooms), batch.rooms);
      await host.store
        .extend(
          batch.rooms,
          batch.since,
          liveAt,
          scope,
          withSeq ? { epoch: snapshot.epoch, seqs } : undefined,
        )
        .catch(() => {});
    }
    if (!count) return { completed: true, ...UNCOUNTED };
    const counted = await this.countNew(
      live.stream,
      live.before,
      liveAt,
      segments,
      walk.events,
    );
    return { completed: true, ...counted };
  }

  /**
   * The new rows of a completed walk (`newRow`). A row stored at or after
   * this connection's start does not count as already stored, whichever
   * path stored it; one received live on this connection is never new.
   */
  private async countNew(
    stream: UserStreamConnection,
    before: number,
    liveAt: number,
    segments: Record<string, [number, number][]> | null,
    events: readonly RelayEvent[],
  ): Promise<Counted> {
    const liveIds = this.liveIds;
    if (!segments || !liveIds) return UNCOUNTED;
    const rows: WalkedRow[] = [];
    const ids: string[] = [];
    for (const event of events) {
      const room = roomTag(event);
      const base = room === null ? undefined : segments[room];
      if (room === null || !base) continue;
      rows.push({
        room,
        kind: event.kind,
        pubkey: event.pubkey,
        liveAt,
        // This connection's own spans are no base (as for resume points).
        segments: base.filter(([lo]) => lo < before),
        inserted: true,
        createdAt: event.created_at,
        receivedLive: liveIds.has(event.id),
      });
      ids.push(event.id);
    }
    // Only rows that would be new if not stored before are asked about.
    const candidates = ids.filter((_, index) => isNewRow(rows[index]));
    const fresh =
      candidates.length === 0
        ? []
        : await this.host.store
            .freshIds(candidates, stream.startedAtMs)
            .catch(() => null);
    if (!fresh) return UNCOUNTED;
    const freshIds = new Set(fresh);
    const newRows: Record<string, number> = {};
    rows.forEach((row, index) => {
      row.inserted = freshIds.has(ids[index]);
      if (isNewRow(row)) newRows[row.room] = (newRows[row.room] ?? 0) + 1;
    });
    return { counted: true, newRows, rows };
  }

  /**
   * Pages from the relay's newest back to `since` (at most 4 pages). No page
   * starts once `stop()`, and each waits for the relay's rate limit.
   */
  private async walk(
    roomIds: string[],
    since: number,
    stop: () => boolean,
  ): Promise<Walk> {
    const { host } = this;
    const limited = host.rateLimited ?? isRateLimited;
    const wait = host.waitForRateLimit ?? waitForRateLimit;
    const events: RelayEvent[] = [];
    let until: number | undefined;
    let beforeId: string | undefined;
    for (let page = 0; page < STORE_CATCH_UP_MAX_PAGES; page++) {
      if (limited()) await wait();
      if (stop()) return { completed: false, events };
      let rows: RelayEvent[];
      try {
        rows = await host.requestPage({
          channelIds: roomIds,
          since,
          limit: STORE_CATCH_UP_PAGE_LIMIT,
          until,
          beforeId,
        });
      } catch {
        return { completed: false, events };
      }
      events.push(...rows);
      const terminal = rows.at(-1);
      if (rows.length < STORE_CATCH_UP_PAGE_LIMIT || !terminal) {
        return { completed: true, events };
      }
      if (terminal.created_at === until && terminal.id === beforeId) break;
      until = terminal.created_at;
      beforeId = terminal.id;
    }
    return { completed: false, events };
  }

  /**
   * Desktop listener repair: rows of `roomIds` since `since` newer than each
   * room's live start minus 5 s reach the listeners, once by id. The room on
   * screen goes first, on its own. Oldest first: a reaction or its removal
   * has no `h`, and its room comes from the row it references.
   */
  private async repair(
    live: LiveConnection,
    roomIds: string[],
    since: number,
    floors: Readonly<Record<string, number>>,
    stop: () => boolean,
  ) {
    const { host } = this;
    const visible = host.visibleRoom?.() ?? null;
    const rest = roomIds.filter((room) => room !== visible);
    const batches = rest.length < roomIds.length ? [[visible as string]] : [];
    for (let start = 0; start < rest.length; start += REPAIR_BATCH_ROOMS) {
      batches.push(rest.slice(start, start + REPAIR_BATCH_ROOMS));
    }
    for (const rooms of batches) {
      if (stop()) return;
      const walk = await this.walk(rooms, since, stop);
      for (const event of [...walk.events].reverse()) {
        const room =
          roomTag(event) ??
          (rooms.length === 1 ? rooms[0] : host.roomOf(event));
        if (room === null) continue;
        const floor =
          floors[room] ??
          (this.liveSince.get(room) ?? live.liveAt) - LISTENER_FLOOR_SKEW_SECS;
        if (event.created_at < floor) continue;
        if (!live.membership.renders(room, event.created_at)) continue;
        host.bundle.dispatchStream(event, room);
      }
    }
  }

  /** A CLOSED after the EOSE in live/gated (`midConnectionClosed`). */
  private async fallback(live: LiveConnection) {
    const { host } = this;
    const closedAt = host.nowSecs();
    // Kept rooms are on their chunks already.
    const carried = [...live.handed];
    // Chunks for every carried room at once (extras are already on theirs).
    host.bundle.setStreamRooms(null);
    const stop = () => live.socketGone;
    const resume = new Map(
      Object.entries(
        await host.store.resumePoints(carried, closedAt).catch(() => ({})),
      ),
    );
    const plan = midConnectionPlan({
      mode: live.snapshot.mode,
      streamLive: true,
      awaitSocketClose: false,
      closedAt,
      carriedRooms: carried,
      chunkExtras: host.bundle.roomLists().extras,
      resume,
      lastSeenCreatedAt: this.lastSeenCreatedAt,
      liveSince: this.liveSince,
    });
    const release = await this.takeWalks();
    try {
      const repair = plan.desktopRepair;
      const repairing = repair
        ? this.repair(
            live,
            repair.rooms,
            repair.since,
            repair.listenerFloor,
            stop,
          )
        : Promise.resolve();
      const fallbackLive = { ...live, liveAt: closedAt, before: closedAt };
      for (const batch of plan.catchUp?.batches ?? []) {
        if (stop()) break;
        await this.walkBatch(fallbackLive, batch, false, false, stop);
      }
      await repairing;
    } finally {
      release();
    }
  }

  private async shadowCatchUp(
    shadow: ShadowConnection,
    snapshot: UserStreamSnapshot,
  ) {
    const { host } = this;
    const queued = await host.store.epoch(snapshot.epoch).catch(() => false);
    const rooms = [...snapshot.rooms.keys()];
    const stored = await host.store
      .seqs(rooms, shadow.stream.startedAt)
      .catch(() => NO_SEQS);
    if (this.shadow !== shadow) return;
    const resume = new Map(stored.vouched ? Object.entries(stored.resume) : []);
    const plan = classifySnapshot(
      "shadow",
      snapshot.rooms,
      rowsOf(stored, snapshot.epoch),
      resume,
    );
    shadow.report.add("predictedSkips", plan.predictedSkips.length);
    if (plan.clearedRows.length > 0) {
      await host.store.clearRows(plan.clearedRows).catch(() => {});
    }
    // Rule W needs the epoch queued first.
    if (!queued || this.shadow !== shadow) return;
    const predicted = new Set(plan.predictedSkips);
    const verified = new Set<string>();
    const seqsOf = Object.fromEntries(snapshot.rooms);
    setStreamCatchUpVouch((roomIds, liveAt) => {
      if (this.shadow !== shadow) return null;
      const seqs = pick(seqsOf, roomIds);
      if (Object.keys(seqs).length === 0) return null;
      const checked = roomIds.filter(
        (room) => predicted.has(room) && !verified.has(room),
      );
      // Read before the walk records its span.
      const segments =
        checked.length === 0
          ? null
          : host.store.segments(checked).catch(() => null);
      return {
        seq: { epoch: snapshot.epoch, seqs },
        ended: (completed, events) => {
          if (!completed || !segments) return;
          void segments
            .then((before) =>
              this.countNew(
                shadow.stream,
                shadow.stream.startedAt,
                liveAt,
                before,
                events,
              ),
            )
            .then((counted) => {
              if (!counted.counted || this.shadow !== shadow) return;
              for (const room of checked) {
                if (verified.has(room)) continue;
                verified.add(room);
                const key =
                  (counted.newRows[room] ?? 0) > 0
                    ? "falseSkip"
                    : "verifiedSkips";
                shadow.report.add(key);
              }
            })
            .catch(() => {});
        },
      };
    });
  }

  private async loadPrevLive(): Promise<PreviousLive | null> {
    const saved = await this.host.store
      .getState(PREV_LIVE_KEY)
      .catch(() => null);
    if (!saved) return null;
    try {
      const prev = JSON.parse(saved) as PreviousLive;
      return Array.isArray(prev.liveRooms) ? prev : null;
    } catch {
      return null;
    }
  }

  /** At the EOSE and every span extension, so a killed app keeps it. */
  private savePrevLive(live: LiveConnection) {
    // A late save of another account's connection.
    if (this.host.report() !== live.report) return Promise.resolve();
    const prev: PreviousLive = {
      liveRooms: [...live.membership.carried],
      eoseAt: live.snapshot.createdAt,
      lastFrameAt: live.stream.lastFrameAt,
    };
    return this.host.store
      .setState(PREV_LIVE_KEY, JSON.stringify(prev))
      .catch(() => {});
  }
}
