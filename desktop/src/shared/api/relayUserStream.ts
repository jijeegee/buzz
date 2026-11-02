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
  readonly backoff = new UserStreamBackoff();
  readonly report: UserStreamReport;
  private current: UserStreamConnection | null = null;
  private graceTimer: number | null = null;
  /** The next reconnect waits a random 0–10 s (after `lagged`). */
  private jitterNext = false;
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
   * replayed. Returns without one when off or backed off.
   */
  async connected(generation: number) {
    this.current = null;
    if (!this.host.enabled()) return;
    if (!this.backoff.allowed(Math.floor(this.host.nowMs() / 1000))) {
      this.report.add("backoffSkipped");
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
    if (stream.mode === "shadow") {
      const room = this.host.roomOf(event);
      if (room) {
        stream.comparator?.observe("stream", event.id, room, this.host.nowMs());
      }
      return;
    }
    this.liveHooks?.frame(stream, event);
  }

  /** An event delivered by a room chunk (shadow comparison). */
  chunkEvent(event: RelayEvent, room: string) {
    const comparator = this.current?.comparator;
    if (!comparator) return;
    comparator.observe("chunk", event.id, room, this.host.nowMs());
  }

  /** A 44100 for `room` on this connection. */
  joined(room: string) {
    this.current?.comparator?.noteJoined(room);
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
    if (!stream) return;
    stream.comparator?.end(this.host.nowMs());
    this.stopComparator(stream);
    if (stream.streamLive && stream.mode !== "shadow" && !stream.closed) {
      stream.closed = true;
      this.liveHooks?.ended(stream, null);
    }
    stream.closed = true;
    this.maybeReport();
  }

  /** Network change: the overflow backoff ends early. */
  networkChange() {
    this.backoff.networkChange();
  }

  /** Back in the foreground after `backgroundSecs`. */
  resumed(backgroundSecs: number) {
    this.backoff.resume(backgroundSecs);
  }

  maybeReport() {
    this.report.maybeReport(this.host.nowMs());
  }

  private stopComparator(stream: UserStreamConnection) {
    const comparator = stream.comparator;
    if (!comparator) return;
    comparator.closed();
    for (const [key, value] of Object.entries(comparator.counters)) {
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
