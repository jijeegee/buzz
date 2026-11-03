/**
 * Pure rules of the per-account user stream: the REQ, the snapshot parts, the
 * relay's CLOSED reasons, the overflow backoff, the history busy retry and
 * the shadow comparator. Rules: docs/message-sync.md#user-stream; cases:
 * test-fixtures/user-stream.json (relayUserStream.test.mjs).
 */
import type { RelayEvent } from "@/shared/api/types";

/** The relay-signed snapshot part, addressed to the account. Never stored. */
export const USER_STREAM_KIND = 24220;
/** The `#h` value naming "every room I belong to". Never a room id. */
export const USER_STREAM_ROOM = "$joined";
/** Sub id prefix of the stream REQ (its own subscription type). */
export const USER_STREAM_SUB_PREFIX = "stream-";

/** Frozen v1 kinds: the relay accepts exactly this set; a change bumps `v`. */
export const USER_STREAM_KINDS: readonly number[] = [
  5, 7, 9, 9005, 39005, 40001, 40002, 40003, 40008, 40009, 40099, 45001, 45003,
  48100, 48101, 48102, 48103,
];

export const USER_STREAM_LIMITS = {
  maxRoomsPerPart: 500,
  catchUpBatchRooms: 128,
  lookbackSecs: 1865,
  catchUpPageLimit: 500,
  catchUpMaxPages: 4,
  catchUpMaxAgeSecs: 604_800,
  compareWindowMs: 5_000,
  auditSampleMax: 128,
  liveMissedAfterEoseSecs: 5,
  liveMissedBeforeLastFrameSecs: 900,
  listenerFloorSecs: 5,
  spanExtendSecs: 60,
  fullClosesForBackoff: 2,
  fullCloseWindowSecs: 600,
  backoffSecs: 3_600,
  backgroundResetSecs: 3_600,
  laggedJitterSecs: [0, 10] as const,
  busyRetryMaxSecs: 30,
  reportChangeRatio: 0.1,
  reportIntervalSecs: 600,
  /** How long the client waits for the relay to end the socket after an
   * overflow or lagged CLOSED in live/gated before falling back on it. */
  socketCloseGraceMs: 2_000,
} as const;

export const USER_STREAM_MAX_EPOCH_CHARS = 64;

export type UserStreamMode = "shadow" | "live" | "gated";
const MODES: readonly string[] = ["shadow", "live", "gated"];

/** The stream REQ's only filter. */
export function userStreamFilter() {
  return {
    "#h": [USER_STREAM_ROOM],
    kinds: [...USER_STREAM_KINDS],
    limit: 0,
  };
}

/** Whether a filter (or any REQ filter) names the user stream's `$joined`. */
export function namesUserStream(filter: unknown): boolean {
  if (!filter || typeof filter !== "object") return false;
  const rooms = (filter as Record<string, unknown>)["#h"];
  return Array.isArray(rooms) && rooms.includes(USER_STREAM_ROOM);
}

/** `#h` room ids of a filter with `$joined` left out (never a room id). */
export function roomIdsOf(filter: { "#h"?: string[] }): string[] | undefined {
  const rooms = filter["#h"];
  return rooms?.filter((room) => room !== USER_STREAM_ROOM);
}

export type UserStreamSnapshot = {
  mode: UserStreamMode;
  epoch: string;
  rooms: Map<string, number>;
  /** Part 1's relay-stamped `created_at` (`prevEose` of liveMissed). */
  createdAt: number;
  /** The key that signed part 1 (the relay). */
  relayPubkey: string;
};

function sameKindSet(kinds: unknown) {
  if (!Array.isArray(kinds)) return false;
  const set = new Set(kinds);
  return (
    set.size === USER_STREAM_KINDS.length &&
    USER_STREAM_KINDS.every((kind) => set.has(kind))
  );
}

function isSeq(value: unknown): value is number {
  return Number.isSafeInteger(value) && (value as number) >= 0;
}

/** A JSON number token the store reads as an integer: digits only. */
const PLAIN_INTEGER = /^(0|[1-9][0-9]*)$/;

/**
 * Parses snapshot content with every number not written as plain digits
 * (`1.0`, `1e0`, `-0`, `-1`) read as `null`, so an integer field holding
 * one is refused exactly as the store's serde check refuses it
 * (docs/message-sync.md#snapshot). `undefined` when it is not JSON.
 */
export function parsePlainIntegers(text: string): unknown {
  try {
    JSON.parse(text);
  } catch {
    return undefined;
  }
  let out = "";
  let index = 0;
  while (index < text.length) {
    const char = text[index];
    if (char === '"') {
      let end = index + 1;
      while (end < text.length && text[end] !== '"') {
        end += text[end] === "\\" ? 2 : 1;
      }
      out += text.slice(index, end + 1);
      index = end + 1;
    } else if (char === "-" || (char >= "0" && char <= "9")) {
      let end = index + 1;
      while (end < text.length && /[0-9.eE+-]/.test(text[end])) end += 1;
      const token = text.slice(index, end);
      out += PLAIN_INTEGER.test(token) ? token : "null";
      index = end;
    } else {
      out += char;
      index += 1;
    }
  }
  return JSON.parse(out);
}

/**
 * Reads the snapshot parts in arrival order. `feed` returns false once the
 * snapshot is refused (CLOSE the stream: chunks for this connection).
 */
export class SnapshotParser {
  ignoredBeforePart1 = 0;
  refused = false;
  private announced = 0;
  private received = 0;
  private part1: Omit<UserStreamSnapshot, "rooms"> | null = null;
  private rooms = new Map<string, number>();

  get mode(): UserStreamMode | null {
    return this.part1?.mode ?? null;
  }

  /** A frame on the stream sub before EOSE. */
  feed(event: Pick<RelayEvent, "kind" | "content"> & Partial<RelayEvent>) {
    if (this.refused) return false;
    if (event.kind !== USER_STREAM_KIND) {
      if (!this.part1) this.ignoredBeforePart1 += 1;
      return true;
    }
    const content = parsePlainIntegers(event.content);
    if (!content || typeof content !== "object" || Array.isArray(content)) {
      return this.refuse();
    }
    const part = content as Record<string, unknown>;
    if (!this.part1) {
      if (
        part.v !== 1 ||
        part.op !== "snapshot" ||
        part.part !== 1 ||
        !Number.isSafeInteger(part.parts) ||
        (part.parts as number) < 1 ||
        typeof part.mode !== "string" ||
        !MODES.includes(part.mode) ||
        typeof part.epoch !== "string" ||
        part.epoch.length === 0 ||
        part.epoch.length > USER_STREAM_MAX_EPOCH_CHARS ||
        !sameKindSet(part.kinds)
      ) {
        return this.refuse();
      }
      this.announced = part.parts as number;
      this.part1 = {
        mode: part.mode as UserStreamMode,
        epoch: part.epoch,
        createdAt: event.created_at ?? 0,
        relayPubkey: event.pubkey ?? "",
      };
    } else if (
      this.received >= this.announced ||
      ("part" in part && part.part !== this.received + 1)
    ) {
      return this.refuse();
    }
    const rooms = part.rooms;
    if (!rooms || typeof rooms !== "object" || Array.isArray(rooms)) {
      return this.refuse();
    }
    for (const [room, seq] of Object.entries(rooms)) {
      if (!isSeq(seq) || this.rooms.has(room)) return this.refuse();
      this.rooms.set(room, seq);
    }
    this.received += 1;
    return true;
  }

  /** EOSE on the stream sub: the snapshot, or `null` when refused. */
  eose(): UserStreamSnapshot | null {
    if (this.refused || !this.part1 || this.received !== this.announced) {
      this.refused = true;
      return null;
    }
    return { ...this.part1, rooms: this.rooms };
  }

  private refuse() {
    this.refused = true;
    return false;
  }
}

const CLOSED_REASONS: readonly {
  match: "exact" | "prefix";
  value: string;
  key: string;
}[] = [
  { match: "exact", value: "unsupported: user-stream off", key: "off" },
  {
    match: "exact",
    value: "unsupported: user-stream not enabled",
    key: "not_enabled",
  },
  { match: "prefix", value: "invalid: user-stream", key: "invalid" },
  {
    match: "exact",
    value: "unsupported: user-stream too many rooms",
    key: "too_many_rooms",
  },
  {
    match: "exact",
    value: "restricted: not a channel member",
    key: "restricted",
  },
  { match: "prefix", value: "auth-required:", key: "auth_required" },
  { match: "exact", value: "error: user-stream overflow", key: "overflow" },
  { match: "exact", value: "error: user-stream lagged", key: "lagged" },
  { match: "exact", value: "error: user-stream closed", key: "closed" },
];

/** The `closed{key}` of a stream CLOSED (`overflow` splits by mode). */
export function closedKey(reason: string, mode: UserStreamMode | null) {
  const rule = CLOSED_REASONS.find(({ match, value }) =>
    match === "prefix" ? reason.startsWith(value) : reason === value,
  );
  if (!rule) return "other";
  if (rule.key === "overflow")
    return mode === "shadow" ? "shadow_full" : "full";
  return rule.key;
}

/**
 * - `chunks`: stream off for this connection; chunks stay or carry it all.
 * - `fallback`: the mid-connection fallback (live/gated after EOSE).
 * - `awaitSocketClose`: the relay ends this socket next; wait for it (the
 *   normal reconnect follows), falling back only if it is still open after
 *   the grace.
 * - `retryAfterAuth`: send the REQ again after AUTH OK, once (before the
 *   stream EOSE, or in shadow).
 */
export type ClosedAction =
  | "chunks"
  | "fallback"
  | "awaitSocketClose"
  | "retryAfterAuth";

export type ClosedDecision = {
  action: ClosedAction;
  key: string;
  countsTowardBackoff: boolean;
  reconnectJitter: boolean;
  bugLog: boolean;
};

/** What a CLOSED on the stream sub means (`closed` cases). */
export function closedDecision({
  reason,
  mode,
  streamLive,
  afterRetry = false,
}: {
  reason: string;
  mode: UserStreamMode | null;
  /** The stream reached an accepted EOSE on this connection. */
  streamLive: boolean;
  afterRetry?: boolean;
}): ClosedDecision {
  const key = closedKey(reason, mode);
  const reliedOn = streamLive && (mode === "live" || mode === "gated");
  const decision: ClosedDecision = {
    action: "chunks",
    key,
    countsTowardBackoff: false,
    reconnectJitter: key === "lagged",
    bugLog: key === "invalid",
  };
  if (key === "auth_required") {
    // After the EOSE in live/gated the relay already dropped frames: a retry
    // would leave that gap unrepaired, so the fallback runs at once.
    if (reliedOn) decision.action = "fallback";
    else decision.action = afterRetry ? "chunks" : "retryAfterAuth";
    return decision;
  }
  // The relay cancels the socket right after these: nothing starts on it.
  if (key === "lagged" || key === "full") {
    decision.action = "awaitSocketClose";
    decision.countsTowardBackoff = key === "full";
    return decision;
  }
  if (!reliedOn) return decision;
  decision.action = "fallback";
  decision.countsTowardBackoff = key === "closed" || key === "other";
  return decision;
}

/**
 * Overflow backoff per account and server: two counting closes within
 * 10 min stop the stream REQ for 1 h from the second. A network change or a
 * resume after at least 1 h in the background ends it and forgets closes.
 */
export class UserStreamBackoff {
  private closes: number[] = [];
  private blockedUntil = 0;

  noteClose(atSecs: number) {
    const window = USER_STREAM_LIMITS.fullCloseWindowSecs;
    this.closes = this.closes.filter((at) => atSecs - at <= window);
    this.closes.push(atSecs);
    if (this.closes.length >= USER_STREAM_LIMITS.fullClosesForBackoff) {
      this.blockedUntil = atSecs + USER_STREAM_LIMITS.backoffSecs;
      this.closes = [];
    }
  }

  networkChange() {
    this.reset();
  }

  resume(backgroundSecs: number) {
    if (backgroundSecs >= USER_STREAM_LIMITS.backgroundResetSecs) this.reset();
  }

  allowed(atSecs: number) {
    return atSecs >= this.blockedUntil;
  }

  /** The state kept in the store's `sync_state` across app restarts. */
  save(): string {
    return JSON.stringify({ closes: this.closes, until: this.blockedUntil });
  }

  restore(saved: string | null) {
    if (!saved) return;
    try {
      const { closes, until } = JSON.parse(saved);
      if (Array.isArray(closes) && closes.every(Number.isFinite)) {
        this.closes = closes;
      }
      if (Number.isFinite(until)) this.blockedUntil = until;
    } catch {
      // A damaged value starts over.
    }
  }

  private reset() {
    this.closes = [];
    this.blockedUntil = 0;
  }
}

/**
 * Which network reports are a real network change for the backoff
 * (docs/message-sync.md#relay-answers): never the first report after start,
 * never a repeat of the same set of network types, never "no network".
 */
export class NetworkChangeFilter {
  private last: ReadonlySet<string> | null = null;

  /** `types`: the network types the OS reports now (empty: offline). */
  changed(types: Iterable<string>) {
    const now = new Set(types);
    const last = this.last;
    this.last = now;
    if (last === null || now.size === 0) return false;
    return now.size !== last.size || [...now].some((type) => !last.has(type));
  }
}

/** Delay before busy retry `attempt` (0-based): 1 s doubling, capped at 30 s. */
export function busyRetryDelaySecs(attempt: number) {
  return Math.min(2 ** attempt, USER_STREAM_LIMITS.busyRetryMaxSecs);
}

/** Newest `min(created_at, receive time)` over a connection's stream frames. */
export function lastFrameAt(
  frames: readonly { created_at: number; receivedAt: number }[],
) {
  let last: number | null = null;
  for (const frame of frames) {
    const at = Math.min(frame.created_at, frame.receivedAt);
    if (last === null || at > last) last = at;
  }
  return last;
}

export type ComparatorCounters = {
  eventsCompared: number;
  chunkOnly: number;
  streamOnlyExpected: number;
  streamOnlyUnexpected: number;
};

type CompareWindow = {
  room: string;
  deadlineMs: number;
  chunk: boolean;
  stream: boolean;
};

/**
 * Shadow comparison from the stream EOSE: each event id is classified once,
 * `compareWindowMs` (inclusive) after it was first seen on either side.
 */
export class ShadowComparator {
  readonly counters: ComparatorCounters = {
    eventsCompared: 0,
    chunkOnly: 0,
    streamOnlyExpected: 0,
    streamOnlyUnexpected: 0,
  };
  private open = new Map<string, CompareWindow>();
  private classified = new Set<string>();
  private joined = new Set<string>();
  private startedAtMs: number | null = null;
  private stopped = false;
  private readonly snapshotRooms: ReadonlySet<string>;
  private readonly chunkRooms: () => ReadonlySet<string>;

  constructor(
    snapshotRooms: Iterable<string>,
    chunkRooms: () => ReadonlySet<string>,
  ) {
    this.snapshotRooms = new Set(snapshotRooms);
    this.chunkRooms = chunkRooms;
  }

  streamEose(atMs: number) {
    this.startedAtMs = atMs;
  }

  /** A 44100 for `room` on this connection: the stream carries it now. */
  noteJoined(room: string) {
    this.joined.add(room);
  }

  observe(on: "chunk" | "stream", id: string, room: string, atMs: number) {
    if (this.stopped || this.startedAtMs === null || atMs < this.startedAtMs) {
      return;
    }
    this.flush(atMs);
    if (this.classified.has(id)) return;
    let window = this.open.get(id);
    if (!window) {
      window = {
        room,
        deadlineMs: atMs + USER_STREAM_LIMITS.compareWindowMs,
        chunk: false,
        stream: false,
      };
      this.open.set(id, window);
    }
    window[on] = true;
  }

  /** Classifies the windows closed by `atMs`. */
  flush(atMs: number) {
    for (const [id, window] of this.open) {
      if (window.deadlineMs >= atMs) continue;
      this.open.delete(id);
      this.classified.add(id);
      this.classify(window);
    }
  }

  /** Stream CLOSED: later frames are ignored, open windows dropped. */
  closed() {
    this.stopped = true;
    this.open.clear();
  }

  /** Connection end at `atMs`: windows still open are dropped. */
  end(atMs: number) {
    this.flush(atMs);
    this.closed();
  }

  private classify(window: CompareWindow) {
    const carried =
      this.snapshotRooms.has(window.room) || this.joined.has(window.room);
    const compared = carried && this.chunkRooms().has(window.room);
    if (compared) this.counters.eventsCompared += 1;
    if (window.chunk && window.stream) return;
    if (window.chunk) {
      if (compared) this.counters.chunkOnly += 1;
      return;
    }
    if (carried) this.counters.streamOnlyExpected += 1;
    else this.counters.streamOnlyUnexpected += 1;
  }
}
