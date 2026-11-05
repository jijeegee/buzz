/**
 * The per-account user stream on one relay session: one REQ for every room
 * of the account, sent first after AUTH OK, its mode chosen by the relay
 * (snapshot part 1). Rules: docs/message-sync.md#user-stream.
 *
 * Without a relay stream (an old relay, `off`, not enabled, backoff) the
 * session behaves exactly as before: chunks carry every room.
 */
import type { RelayEvent } from "@/shared/api/types";
import {
  closedDecision,
  ShadowComparator,
  SnapshotParser,
  USER_STREAM_KIND,
  USER_STREAM_LIMITS,
  USER_STREAM_SUB_PREFIX,
  UserStreamBackoff,
  userStreamFilter,
  type ClosedDecision,
  type UserStreamMode,
  type UserStreamSnapshot,
} from "@/shared/api/userStreamRules";

/** Build flag: `off` never sends the REQ; `follow` follows the relay. */
export function userStreamEnabled() {
  const flag = import.meta.env?.VITE_KUZZ_USER_STREAM;
  return flag !== "off";
}

export type UserStreamHost = {
  /** The session's current connection generation. */
  generation: () => number;
  send: (payload: unknown[], generation: number) => Promise<void>;
  /** Ends the socket of `generation` (the normal reconnect follows). */
  closeSocket: (generation: number) => void;
  /** Rooms a live chunk carries now (shadow comparison). */
  chunkRooms: () => ReadonlySet<string>;
  /** Room of a stream frame (`h` tag or a referenced event). */
  roomOf: (event: RelayEvent) => string | null;
  nowMs: () => number;
  setTimeout: (fn: () => void, ms: number) => number;
  clearTimeout: (id: number) => void;
  log: (line: string) => void;
  enabled: () => boolean;
  random?: () => number;
  /** The store's stream `sync_state` values (backoff, counters). */
  loadState?: (key: string) => Promise<string | null>;
  saveState?: (key: string, value: string) => void;
};

/** Hooks the live/gated handoff installs (C2d); absent in shadow. */
export type UserStreamLiveHooks = {
  /** Accepted live/gated EOSE: the stream carries `snapshot` from now on. */
  eose: (stream: UserStreamConnection, snapshot: UserStreamSnapshot) => void;
  /** A frame after the EOSE in live/gated. */
  frame: (stream: UserStreamConnection, event: RelayEvent) => void;
  /** The stream stopped on a live connection (CLOSED, socket end). */
  ended: (
    stream: UserStreamConnection,
    decision: ClosedDecision | null,
  ) => void;
  /**
   * The socket ended: chunks carry every room from the next connect.
   * `startedAt` (s): when that connection reached AUTH OK, given once.
   */
  socketEnded: (startedAt?: number | null) => void;
  /** A live 44100/44101 for the account. */
  membershipNotice: (event: RelayEvent) => void;
  /** Accepted shadow EOSE: classes and verifies skips, walks nothing. */
  shadowEose?: (
    stream: UserStreamConnection,
    snapshot: UserStreamSnapshot,
  ) => void;
  /** An event received live on this connection (stream frame). */
  noteLive?: (eventId: string) => void;
  /** Live ids are collected only while the stream REQ is pending or accepted. */
  collectLiveIds?: (on: boolean) => void;
  /** Another account or community: nothing in memory carries over. */
  scopeChanged?: () => void;
};

/** One stream REQ on one connection. */
export type UserStreamConnection = {
  generation: number;
  subId: string;
  parser: SnapshotParser;
  snapshot: UserStreamSnapshot | null;
  mode: UserStreamMode | null;
  /** The stream reached an accepted EOSE. */
  streamLive: boolean;
  closed: boolean;
  authRetried: boolean;
  /** Waiting for the next AUTH OK to send the REQ again. */
  awaitingAuth: boolean;
  /** Newest `min(created_at, receive time)` over its frames (`lastFrame`). */
  lastFrameAt: number | null;
  /**
   * When the socket's connection began (s, before any chunk REQ answered):
   * spans recorded on this connection are not a catch-up's resume point.
   */
  startedAt: number;
  /** The same start in ms on this device's clock (rows stored since). */
  startedAtMs: number;
  comparator: ShadowComparator | null;
};

type Counters = Record<string, number>;

/**
 * `[user-stream] report #<seq> <json>` on a change of at least 10% (any
 * change from 0), or 10 min after the last line if anything changed.
 */
export class UserStreamReport {
  counters: Counters = {};
  private reported: Counters = {};
  private seq = 0;
  private lastAtMs: number | null = null;
  private readonly log: (line: string) => void;

  constructor(log: (line: string) => void) {
    this.log = log;
  }

  /** Cumulative counters and the report sequence, kept in `sync_state`. */
  save() {
    return JSON.stringify({ seq: this.seq, counters: this.counters });
  }

  restore(saved: string | null) {
    if (!saved) return;
    try {
      const { seq, counters } = JSON.parse(saved);
      if (!Number.isInteger(seq) || typeof counters !== "object") return;
      for (const [key, value] of Object.entries(counters ?? {})) {
        if (Number.isFinite(value)) {
          this.counters[key] = (this.counters[key] ?? 0) + (value as number);
        }
      }
      this.reported = { ...this.counters };
      this.seq = Math.max(this.seq, seq);
    } catch {
      // A damaged value starts over.
    }
  }

  add(key: string, by = 1) {
    if (by === 0) return;
    this.counters[key] = (this.counters[key] ?? 0) + by;
  }

  maybeReport(nowMs: number) {
    let changed = false;
    let big = false;
    for (const [key, value] of Object.entries(this.counters)) {
      const before = this.reported[key] ?? 0;
      if (value === before) continue;
      changed = true;
      if (
        before === 0 ||
        Math.abs(value - before) >=
          USER_STREAM_LIMITS.reportChangeRatio * before
      ) {
        big = true;
      }
    }
    const due =
      this.lastAtMs !== null &&
      nowMs - this.lastAtMs >= USER_STREAM_LIMITS.reportIntervalSecs * 1000;
    if (!changed || (!big && !due)) return false;
    this.seq += 1;
    this.lastAtMs = nowMs;
    this.reported = { ...this.counters };
    this.log(
      `[user-stream] report #${this.seq} ${JSON.stringify(this.counters)}`,
    );
    return true;
  }
}

export class UserStreamSession {
  backoff = new UserStreamBackoff();
  report: UserStreamReport;
  private current: UserStreamConnection | null = null;
  private graceTimer: number | null = null;
  /** The next reconnect waits a random 0–10 s (after `lagged`). */
  private jitterNext = false;
  /** Backoff and counters were read from the store for this scope. */
  private restored = false;
  /** Bumped per `connected`, so a late restore starts no stale REQ. */
  private attempt = 0;
  /** When the current connection reached AUTH OK (s), until it ends. */
  private connectedAt: number | null = null;
  /** The last `connected`'s stream REQ, sent or given up (tests). */
  requested: Promise<void> = Promise.resolve();
  liveHooks: UserStreamLiveHooks | null = null;
  private readonly host: UserStreamHost;

  constructor(host: UserStreamHost) {
    this.host = host;
    this.report = new UserStreamReport(host.log);
  }

  /** The stream of the current connection, if one was requested. */
  get connection() {
    return this.current;
  }

  handles(subId: string) {
    return this.current?.subId === subId;
  }

  /** Whether `subId` names a stream REQ (never a room chunk). */
  static isStreamSub(subId: string) {
    return subId.startsWith(USER_STREAM_SUB_PREFIX);
  }

  /**
   * AUTH OK on `generation`: sends the stream REQ, before any chunk is
   * replayed. Returns without one when off or backed off. The first connect
   * of a scope reads the backoff and counters first; chunks do not wait for
   * that read, and the REQ follows when it lands.
   */
  connected(generation: number): Promise<void> {
    this.current = null;
    // Whatever the last connection carried is on chunks again, also when
    // this one is backed off or off.
    this.liveHooks?.socketEnded(this.takeConnectedAt());
    const startedAtMs = this.host.nowMs();
    this.connectedAt = Math.floor(startedAtMs / 1000);
    const attempt = ++this.attempt;
    if (!this.host.enabled()) return Promise.resolve();
    this.liveHooks?.collectLiveIds?.(true);
    if (!this.restored && this.host.loadState) {
      this.restored = true;
      this.requested = this.restore(this.host.loadState).then((current) => {
        if (current) return this.request(generation, attempt, startedAtMs);
      });
      return Promise.resolve();
    }
    this.requested = this.request(generation, attempt, startedAtMs);
    return this.requested;
  }

  /** Backoff and counters from the store; `false` when the scope changed. */
  private async restore(load: (key: string) => Promise<string | null>) {
    const { backoff, report } = this;
    const [savedBackoff, counters] = await Promise.all([
      load("stream_backoff").catch(() => null),
      load("stream_counters").catch(() => null),
    ]);
    // Another account or connection since: its values are not this one's.
    if (this.backoff !== backoff || this.report !== report) return false;
    backoff.restore(savedBackoff);
    report.restore(counters);
    return true;
  }

  private async request(
    generation: number,
    attempt: number,
    startedAtMs: number,
  ) {
    // A newer connect since (a late restore): its REQ is that one's.
    if (attempt !== this.attempt) return;
    const startedAt = Math.floor(startedAtMs / 1000);
    if (!this.backoff.allowed(Math.floor(this.host.nowMs() / 1000))) {
      this.report.add("backoffSkipped");
      this.liveHooks?.collectLiveIds?.(false);
      return;
    }
    const stream: UserStreamConnection = {
      generation,
      subId: `${USER_STREAM_SUB_PREFIX}${crypto.randomUUID()}`,
      parser: new SnapshotParser(),
      snapshot: null,
      mode: null,
      streamLive: false,
      closed: false,
      authRetried: false,
      awaitingAuth: false,
      lastFrameAt: null,
      startedAt,
      startedAtMs,
      comparator: null,
    };
    this.current = stream;
    this.report.add("requested");
    await this.sendReq(stream).catch(() => {});
  }

  /** The next reconnect's extra wait in ms (0 unless after `lagged`). */
  takeReconnectJitterMs() {
    if (!this.jitterNext) return 0;
    this.jitterNext = false;
    const [low, high] = USER_STREAM_LIMITS.laggedJitterSecs;
    const random = this.host.random ?? Math.random;
    return Math.round((low + random() * (high - low)) * 1000);
  }

  /** AUTH OK again mid-connection: the once-per-connection REQ retry. */
  authOk(generation: number) {
    const stream = this.current;
    if (!stream || stream.generation !== generation || !stream.awaitingAuth) {
      return;
    }
    stream.awaitingAuth = false;
    stream.closed = false;
    stream.parser = new SnapshotParser();
    void this.sendReq(stream).catch(() => {});
  }

  event(subId: string, event: RelayEvent) {
    const stream = this.current;
    if (!stream || stream.subId !== subId || stream.closed) return;
    const receivedAt = Math.floor(this.host.nowMs() / 1000);
    const at = Math.min(event.created_at, receivedAt);
    if (stream.lastFrameAt === null || at > stream.lastFrameAt) {
      stream.lastFrameAt = at;
    }
    if (!stream.streamLive) {
      const before = stream.parser.mode;
      if (!stream.parser.feed(event)) {
        this.report.add("snapshotRefused");
        this.closeStream(stream);
        return;
      }
      if (before === null && stream.parser.mode !== null) {
        stream.mode = stream.parser.mode;
        this.report.add(`mode_${stream.mode}`);
        if (stream.mode !== "shadow" && !this.liveHooks) {
          // No live/gated handoff in this build: chunks keep every room.
          this.report.add("liveUnsupported");
          this.closeStream(stream);
        }
      }
      return;
    }
    if (event.kind === USER_STREAM_KIND) return;
    this.liveHooks?.noteLive?.(event.id);
    if (stream.mode === "shadow") {
      const room = this.host.roomOf(event);
      if (room) {
        stream.comparator?.observe(
          "stream",
          event.id,
          room,
          this.host.nowMs(),
          event,
        );
      }
      return;
    }
    this.liveHooks?.frame(stream, event);
  }

  /** An event delivered by a room chunk (shadow comparison). */
  chunkEvent(event: RelayEvent, room: string) {
    const comparator = this.current?.comparator;
    if (!comparator) return;
    comparator.observe("chunk", event.id, room, this.host.nowMs(), event);
  }

  /** A 44100 for `room` on this connection. */
  joined(room: string) {
    this.current?.comparator?.noteJoined(room);
  }

  /** A live 44100/44101 for the account (`membership`). */
  membershipNotice(event: RelayEvent) {
    const stream = this.current;
    if (!stream || stream.closed || !stream.streamLive) return;
    const room = event.tags.find((tag) => tag[0] === "h")?.[1];
    if (event.kind === 44100 && room) this.joined(room);
    if (stream.mode !== "shadow") this.liveHooks?.membershipNotice(event);
  }

  eose(subId: string) {
    const stream = this.current;
    if (!stream || stream.subId !== subId || stream.closed) return;
    if (stream.streamLive) return;
    this.report.add("ignoredBeforePart1", stream.parser.ignoredBeforePart1);
    const snapshot = stream.parser.eose();
    if (!snapshot) {
      this.report.add("snapshotRefused");
      this.closeStream(stream);
      return;
    }
    stream.snapshot = snapshot;
    stream.mode = snapshot.mode;
    stream.streamLive = true;
    this.report.add("streamEose");
    if (snapshot.mode === "shadow") {
      stream.comparator = new ShadowComparator(snapshot.rooms.keys(), () =>
        this.host.chunkRooms(),
      );
      stream.comparator.streamEose(this.host.nowMs());
      this.report.add("snapshotRooms", snapshot.rooms.size);
      this.liveHooks?.shadowEose?.(stream, snapshot);
      return;
    }
    this.liveHooks?.eose(stream, snapshot);
  }

  closed(subId: string, reason: string) {
    const stream = this.current;
    if (!stream || stream.subId !== subId) return;
    const decision = closedDecision({
      reason,
      mode: stream.mode,
      streamLive: stream.streamLive,
      afterRetry: stream.authRetried,
    });
    this.report.add(`closed_${decision.key}`);
    if (decision.bugLog) {
      console.error("[user-stream] relay refused the stream REQ", reason);
    }
    if (decision.countsTowardBackoff) {
      this.backoff.noteClose(Math.floor(this.host.nowMs() / 1000));
      this.saveBackoff();
    }
    if (decision.reconnectJitter) this.jitterNext = true;
    stream.closed = true;
    this.stopComparator(stream);
    if (decision.action === "retryAfterAuth") {
      stream.authRetried = true;
      stream.awaitingAuth = true;
      return;
    }
    if (decision.action === "awaitSocketClose") {
      this.awaitSocketClose(stream);
    }
    // Refused before its EOSE: this connection counts nothing.
    if (!stream.streamLive) this.liveHooks?.collectLiveIds?.(false);
    if (stream.streamLive && stream.mode !== "shadow") {
      this.liveHooks?.ended(stream, decision);
    }
    this.maybeReport();
  }

  /** The connection ended (socket closed or reset, community switch). */
  socketEnded() {
    const stream = this.current;
    this.current = null;
    if (this.graceTimer !== null) {
      this.host.clearTimeout(this.graceTimer);
      this.graceTimer = null;
    }
    const startedAt = this.takeConnectedAt();
    if (!stream) {
      this.liveHooks?.socketEnded(startedAt);
      return;
    }
    stream.comparator?.end(this.host.nowMs());
    this.stopComparator(stream);
    if (stream.streamLive && stream.mode !== "shadow" && !stream.closed) {
      stream.closed = true;
      this.liveHooks?.ended(stream, null);
    }
    stream.closed = true;
    this.liveHooks?.socketEnded(startedAt);
    this.maybeReport();
  }

  private takeConnectedAt() {
    const at = this.connectedAt;
    this.connectedAt = null;
    return at;
  }

  /** Another account or community: read its backoff and counters anew. */
  scopeChanged() {
    this.liveHooks?.scopeChanged?.();
    this.restored = false;
    this.report = new UserStreamReport(this.host.log);
    this.backoff = new UserStreamBackoff();
  }

  /** Network change: the overflow backoff ends early. */
  networkChange() {
    this.backoff.networkChange();
    this.saveBackoff();
  }

  /** Back in the foreground after `backgroundSecs`. */
  resumed(backgroundSecs: number) {
    this.backoff.resume(backgroundSecs);
    this.saveBackoff();
  }

  maybeReport() {
    if (this.report.maybeReport(this.host.nowMs())) {
      this.host.saveState?.("stream_counters", this.report.save());
    }
  }

  private saveBackoff() {
    this.host.saveState?.("stream_backoff", this.backoff.save());
  }

  private stopComparator(stream: UserStreamConnection) {
    const comparator = stream.comparator;
    if (!comparator) return;
    comparator.closed();
    for (const [key, value] of Object.entries({
      ...comparator.counters,
      ...comparator.chunkOnlyDetail,
    })) {
      this.report.add(key, value);
    }
    stream.comparator = null;
  }

  private sendReq(stream: UserStreamConnection) {
    return this.host.send(
      ["REQ", stream.subId, userStreamFilter()],
      stream.generation,
    );
  }

  /** Client CLOSE: chunks for the rest of this connection. */
  private closeStream(stream: UserStreamConnection) {
    stream.closed = true;
    this.stopComparator(stream);
    if (!stream.streamLive) this.liveHooks?.collectLiveIds?.(false);
    void this.host
      .send(["CLOSE", stream.subId], stream.generation)
      .catch(() => {});
  }

  /**
   * After an overflow or `lagged` CLOSED the relay ends the socket; nothing
   * starts on it. Still open after the grace: the client ends it itself.
   */
  private awaitSocketClose(stream: UserStreamConnection) {
    if (this.graceTimer !== null) this.host.clearTimeout(this.graceTimer);
    this.graceTimer = this.host.setTimeout(() => {
      this.graceTimer = null;
      if (this.host.generation() !== stream.generation) return;
      this.host.closeSocket(stream.generation);
    }, USER_STREAM_LIMITS.socketCloseGraceMs);
  }
}
