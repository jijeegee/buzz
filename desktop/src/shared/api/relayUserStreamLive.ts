/**
 * The user stream in `live`/`gated` (docs/message-sync.md#user-stream):
 * handoff after the stream EOSE, routing of its frames, the batched
 * catch-up with rule-W watermark writes, the gated skip and its audit,
 * `liveMissed`, the listener repair, the mid-connection fallback and the
 * 44100/44101 hooks. The rules themselves are in `userStreamCatchUp.ts`.
 */
import type { ChannelReconnectRepairRequest } from "@/shared/api/channelReconnectRepair";
import {
  endStreamCoverage,
  noteStreamLiveEose,
  STORE_CATCH_UP_MAX_PAGES,
  STORE_CATCH_UP_PAGE_LIMIT,
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

export type WalkBaseline = {
  segments: [number, number][];
  ids: string[];
};

/** The device store as the stream uses it (`message_store_*` commands). */
export type StreamStore = {
  /** Queues an accepted epoch ahead of this connection's rule-W writes. */
  epoch: (epoch: string) => Promise<boolean>;
  seqs: (roomIds: string[]) => Promise<{
    epoch: string | null;
    rows: Record<string, [number, number]>;
  }>;
  clearRows: (roomIds: string[]) => Promise<void>;
  scope: (subId?: string) => Promise<string | null>;
  resumePoints: (
    roomIds: string[],
    before: number,
  ) => Promise<Record<string, number>>;
  baseline: (
    roomIds: string[],
    from: number,
    to: number,
  ) => Promise<Record<string, WalkBaseline> | null>;
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
  /** `live: false`: off chunks, but not live (a stopped stream's wait). */
  setStreamRooms: (rooms: ReadonlySet<string> | null, live?: boolean) => void;
  streamRoomsChanged: () => void;
  /** Delivers once by id; `false` when the room is unknown here. */
  dispatchStream: (event: RelayEvent, roomId: string) => boolean;
  delivered: (eventId: string) => boolean;
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
};

type Walk = { completed: boolean; events: RelayEvent[] };

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
  span: StreamSpan;
  coverage: ReturnType<typeof noteStreamLiveEose>;
  ended: boolean;
};

export class UserStreamLive implements UserStreamLiveHooks {
  private current: LiveConnection | null = null;
  /** Newest stream frame `created_at` (in memory, as for chunks). */
  private lastSeenCreatedAt: number | null = null;
  /** When the stream first carried each room in this app run. */
  private readonly liveSince = new Map<string, number>();
  private readonly host: UserStreamLiveHost;

  constructor(host: UserStreamLiveHost) {
    this.host = host;
  }

  /** The catch-up started at the last EOSE (tests). */
  settled: Promise<void> = Promise.resolve();

  eose(stream: UserStreamConnection, snapshot: UserStreamSnapshot) {
    const { host } = this;
    const liveAt = host.nowSecs();
    // Queued before any rule-W write of this connection.
    const epochQueued = host.store.epoch(snapshot.epoch).catch(() => false);
    const scope = host.store.scope(stream.subId).catch(() => null);
    const membership = new StreamMembership(snapshot.rooms.keys());
    const span = new StreamSpan();
    span.eose(liveAt);
    const coverage = noteStreamLiveEose({
      generation: stream.generation,
      liveAt,
      rooms: membership.carried,
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
      span,
      coverage,
      ended: false,
    };
    this.current = live;
    for (const room of membership.carried) {
      if (!this.liveSince.has(room)) this.liveSince.set(room, liveAt);
    }
    host.bundle.setStreamRooms(membership.carried);
    report.add("snapshotRooms", snapshot.rooms.size);
    const repairSince =
      this.lastSeenCreatedAt === null
        ? null
        : Math.max(0, this.lastSeenCreatedAt - USER_STREAM_LIMITS.lookbackSecs);
    this.settled = this.catchUp(live, epochQueued, repairSince).catch(
      (error: unknown) => host.log(`[user-stream] catch-up failed ${error}`),
    );
    coverage.settle(this.settled);
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
    if (!room) return;
    // Always stored (by the store layer); shown unless left (44101).
    if (!live.membership.renders(room, event.created_at)) return;
    if (!host.bundle.dispatchStream(event, room)) host.listRefresh();
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
      this.host.bundle.setStreamRooms(live.membership.carried, false);
      return;
    }
    live.report.add(`fallback_${decision.key}`);
    void this.fallback(live).catch((error: unknown) =>
      this.host.log(`[user-stream] fallback failed ${error}`),
    );
  }

  /** The socket ended: every room is on chunks again from the next connect. */
  socketEnded() {
    const live = this.current;
    this.current = null;
    if (live && !live.ended) {
      live.ended = true;
      live.span.closed();
      endStreamCoverage();
    }
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
      this.host.bundle.streamRoomsChanged();
    } else if (event.kind === KIND_MEMBER_ADDED) {
      const from = live.membership.joined(
        room,
        event.created_at,
        this.host.nowSecs(),
      );
      if (from === null) return;
      if (!this.liveSince.has(room)) this.liveSince.set(room, from);
      live.coverage.carry(room, from);
      this.host.bundle.streamRoomsChanged();
    }
  }

  private async catchUp(
    live: LiveConnection,
    epochQueued: Promise<boolean>,
    repairSince: number | null,
  ) {
    const { host } = this;
    const { snapshot, liveAt } = live;
    const prev = await this.loadPrevLive();
    await this.savePrevLive(live);
    await epochQueued;
    const rooms = [...snapshot.rooms.keys()];
    const stored = await host.store
      .seqs(rooms)
      .catch(() => ({ epoch: null, rows: {} }));
    // Rows of another epoch are none (its clear may not be written yet).
    const rows = new Map<string, number>();
    if (stored.epoch === snapshot.epoch) {
      for (const [room, [seq]] of Object.entries(stored.rows)) {
        rows.set(room, seq);
      }
    }
    // Coverage from before this connection only: a chunk EOSE span `[C, C]`
    // recorded on it would hide an offline gap from the walk and the rows.
    const resume = new Map(
      Object.entries(
        await host.store.resumePoints(rooms, live.before).catch(() => ({})),
      ),
    );
    const plan = classifySnapshot(snapshot.mode, snapshot.rooms, rows, resume);
    const { report } = live;
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
    const { batches } = planCatchUpBatches(walkResume, liveAt);
    const walked = new Map<string, boolean>();
    const newRows: Record<string, number> = {};
    const walkedRows: WalkedRow[] = [];
    for (const batch of batches) {
      const result = await this.walkBatch(live, batch, true);
      for (const room of batch.rooms) walked.set(room, result.completed);
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
    const audited = await this.audit(live, plan.skip, resume);
    if (prev) {
      const missed = liveMissed({
        prev,
        walked,
        rows: walkedRows,
        relayPubkey: snapshot.relayPubkey,
      });
      report.add("liveRoomsChecked", missed.liveRoomsChecked);
      report.add("liveMissed", missed.liveMissed);
    }
    if (repairSince !== null) {
      const repaired = [...batches.flatMap((batch) => batch.rooms), ...audited];
      await this.repair(live, repaired, repairSince);
    }
  }

  /** Gated: walk a sample of the skipped rooms (`auditSampling`). */
  private async audit(
    live: LiveConnection,
    skipped: readonly string[],
    resume: ReadonlyMap<string, number>,
  ) {
    if (live.snapshot.mode !== "gated" || skipped.length === 0) return [];
    const skippedResume = new Map(
      skipped.flatMap((room) => {
        const at = resume.get(room);
        return at === undefined ? [] : [[room, at] as [string, number]];
      }),
    );
    const eligible = auditEligible(skippedResume, live.liveAt);
    const sample = auditSample(eligible, this.host.random);
    const sampled = new Map(
      sample.map((room) => [room, skippedResume.get(room) ?? 0]),
    );
    const results = [];
    for (const batch of planCatchUpBatches(sampled, live.liveAt).batches) {
      const result = await this.walkBatch(live, batch, true);
      results.push({
        rooms: batch.rooms,
        completed: result.completed,
        newRows: result.newRows,
      });
    }
    const verdict = auditVerdicts(results);
    const { report } = live;
    report.add("skipAudited", verdict.skipAudited);
    report.add("skipAuditNew", verdict.skipAuditNew);
    report.add("skipAuditFailed", verdict.skipAuditFailed);
    return sample;
  }

  /**
   * One catch-up batch up to the stream EOSE (L). A completed walk records
   * its span, with rule-W rows when `withSeq` (rule W: it started after
   * this connection's stream EOSE), and counts its new rows.
   */
  private async walkBatch(
    live: LiveConnection,
    batch: CatchUpBatch,
    withSeq: boolean,
  ) {
    const { host } = this;
    const { liveAt, snapshot } = live;
    const baseline = await host.store
      .baseline(batch.rooms, batch.since, liveAt)
      .catch(() => null);
    const walk = await this.walk(batch.rooms, batch.since);
    const newRows: Record<string, number> = {};
    const rows: WalkedRow[] = [];
    if (!walk.completed) return { completed: false, newRows, rows };
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
    if (!baseline) return { completed: true, newRows, rows };
    const known = new Map(
      Object.entries(baseline).map(([room, base]) => [room, new Set(base.ids)]),
    );
    for (const event of walk.events) {
      const room = roomTag(event);
      const base = room === null ? undefined : baseline[room];
      if (room === null || !base) continue;
      const row: WalkedRow = {
        room,
        kind: event.kind,
        pubkey: event.pubkey,
        liveAt,
        // This connection's own spans are no base (B1, as for resume points).
        segments: base.segments.filter(([lo]) => lo < live.before),
        inserted: !known.get(room)?.has(event.id),
        createdAt: event.created_at,
        receivedLive: host.bundle.delivered(event.id),
      };
      rows.push(row);
      if (isNewRow(row)) newRows[room] = (newRows[room] ?? 0) + 1;
    }
    return { completed: true, newRows, rows };
  }

  /** Pages from the relay's newest back to `since` (at most 4 pages). */
  private async walk(roomIds: string[], since: number): Promise<Walk> {
    const events: RelayEvent[] = [];
    let until: number | undefined;
    let beforeId: string | undefined;
    for (let page = 0; page < STORE_CATCH_UP_MAX_PAGES; page++) {
      let rows: RelayEvent[];
      try {
        rows = await this.host.requestPage({
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
   * room's live start minus 5 s reach the listeners, once by id.
   */
  private async repair(
    live: LiveConnection,
    roomIds: string[],
    since: number,
    floors: Readonly<Record<string, number>> = {},
  ) {
    for (let start = 0; start < roomIds.length; start += 128) {
      const rooms = roomIds.slice(start, start + 128);
      const walk = await this.walk(rooms, since);
      for (const event of walk.events) {
        const room = roomTag(event);
        if (room === null) continue;
        const floor =
          floors[room] ??
          (this.liveSince.get(room) ?? live.liveAt) - LISTENER_FLOOR_SKEW_SECS;
        if (event.created_at < floor) continue;
        if (!live.membership.renders(room, event.created_at)) continue;
        this.host.bundle.dispatchStream(event, room);
      }
    }
  }

  /** A CLOSED after the EOSE in live/gated (`midConnectionClosed`). */
  private async fallback(live: LiveConnection) {
    const { host } = this;
    const closedAt = host.nowSecs();
    const carried = [...live.membership.carried];
    // Chunks for every carried room at once (extras are already on theirs).
    host.bundle.setStreamRooms(null);
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
    const fallbackLive = { ...live, liveAt: closedAt, before: closedAt };
    for (const batch of plan.catchUp?.batches ?? []) {
      await this.walkBatch(fallbackLive, batch, false);
    }
    if (plan.desktopRepair) {
      await this.repair(
        live,
        plan.desktopRepair.rooms,
        plan.desktopRepair.since,
        plan.desktopRepair.listenerFloor,
      );
    }
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
