import type { RelayEvent } from "@/shared/api/types";

/**
 * Channel order numbers (`channel_sequence`) from a relay that speaks
 * `kuzz-seq` (NIP-11 `supported_extensions`; the socket connects with
 * `?kuzz_seq=1`).
 *
 * Live numbers are not dense for one subscriber (kind filters and hidden rows
 * skip numbers), so a jump proves nothing. And a gap can be signalled after
 * live events already moved the newest number past it. Repair therefore
 * starts from a number that was already the newest {@link SEQ_STABLE_AFTER_MS}
 * ago (the same rule as kuzz-acp), or just below the lowest number seen when
 * the cursor is younger than that. Events fetched twice are dropped by id.
 */
export const SEQ_STABLE_AFTER_MS = 30_000;
/** Page size for `after_seq` repair (the relay caps it at 500). */
export const AFTER_SEQ_PAGE_LIMIT = 500;
/** Pages one repair pass may fetch before it leaves a pin for the next one. */
export const AFTER_SEQ_MAX_PAGES = 20;
export const KUZZ_SEQ_EXTENSION = "kuzz-seq";
const BOUNDS_KIND = 39006;

export class SeqCursor {
  private newest: number | undefined;
  private lowest: number | undefined;
  private stable: number | undefined;
  private candidate: number | undefined;
  private candidateAt: number | undefined;
  /** Where the next repair must start: an unfinished pass left it. */
  private pin: number | undefined;

  observe(seq: number, now: number) {
    this.promote(now);
    if (this.candidate === undefined && this.newest !== undefined) {
      this.candidate = this.newest;
      this.candidateAt = now;
    }
    this.newest = this.newest === undefined ? seq : Math.max(this.newest, seq);
    this.lowest = this.lowest === undefined ? seq : Math.min(this.lowest, seq);
  }

  /** The `after_seq` to repair from, or undefined when no number is known. */
  replayPoint(now: number): number | undefined {
    const stable = this.stableAt(now);
    const base =
      stable ??
      (this.lowest === undefined ? undefined : Math.max(0, this.lowest - 1));
    if (this.pin === undefined) return base;
    return base === undefined ? this.pin : Math.min(this.pin, base);
  }

  /** A repair pass from `seq` started or stopped there; resume no later. */
  pinAt(seq: number) {
    this.pin = this.pin === undefined ? seq : Math.min(this.pin, seq);
  }

  /** A pass moved its unfinished point forward to `seq`. */
  movePin(seq: number) {
    this.pin = seq;
  }

  clearPin() {
    this.pin = undefined;
  }

  private stableAt(now: number) {
    if (
      this.candidate !== undefined &&
      this.candidateAt !== undefined &&
      now - this.candidateAt >= SEQ_STABLE_AFTER_MS
    ) {
      return this.stable === undefined
        ? this.candidate
        : Math.max(this.stable, this.candidate);
    }
    return this.stable;
  }

  private promote(now: number) {
    const stable = this.stableAt(now);
    if (stable !== this.stable) {
      this.stable = stable;
      this.candidate = undefined;
      this.candidateAt = undefined;
    }
  }
}

/** Per-channel cursors for one relay session (kept across reconnects). */
export class SeqCursors {
  private cursors = new Map<string, SeqCursor>();

  get(channelId: string) {
    return this.cursors.get(channelId);
  }

  observe(channelId: string, seq: number, now = Date.now()) {
    let cursor = this.cursors.get(channelId);
    if (!cursor) {
      cursor = new SeqCursor();
      this.cursors.set(channelId, cursor);
    }
    cursor.observe(seq, now);
  }

  replayPoint(channelId: string, now = Date.now()) {
    return this.cursors.get(channelId)?.replayPoint(now);
  }

  clear() {
    this.cursors.clear();
  }
}

/** The trailing `{channel_sequence:N}` of an opted-in EVENT/OK frame. */
export function frameChannelSequence(meta: unknown): number | undefined {
  if (!meta || typeof meta !== "object") return undefined;
  const value = (meta as { channel_sequence?: unknown }).channel_sequence;
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0
    ? value
    : undefined;
}

/** `?kuzz_seq=1` for the socket only; AUTH keeps signing the bare URL. */
export function withKuzzSeqParam(url: string) {
  return `${url}${url.includes("?") ? "&" : "?"}kuzz_seq=1`;
}

export type AfterSeqBounds = {
  lastSeq: number;
  hasMore: boolean;
  sequences: Map<string, number>;
};

/**
 * `Some` when `event` is the bounds event closing an `after_seq` page. Kind
 * 39006 is shared with the channel-window bounds; only `order: "seq"` (and a
 * `:after_seq:` `d` tag) marks this one.
 */
export function afterSeqBounds(event: RelayEvent): AfterSeqBounds | undefined {
  if (event.kind !== BOUNDS_KIND) return undefined;
  let content: unknown;
  try {
    content = JSON.parse(event.content);
  } catch {
    return undefined;
  }
  if (!content || typeof content !== "object") return undefined;
  const body = content as {
    order?: unknown;
    last_seq?: unknown;
    has_more?: unknown;
    sequences?: unknown;
  };
  if (body.order !== "seq" || typeof body.last_seq !== "number")
    return undefined;
  const sequences = new Map<string, number>();
  if (Array.isArray(body.sequences)) {
    for (const pair of body.sequences) {
      if (
        Array.isArray(pair) &&
        typeof pair[0] === "string" &&
        typeof pair[1] === "number"
      ) {
        sequences.set(pair[0], pair[1]);
      }
    }
  }
  return {
    lastSeq: body.last_seq,
    hasMore: body.has_more === true,
    sequences,
  };
}

/** A 39006 that closes an `after_seq` page; never a live channel event. */
export function isAfterSeqBoundsEvent(event: RelayEvent) {
  return (
    event.kind === BOUNDS_KIND &&
    (event.tags.some(
      (tag) => tag[0] === "d" && (tag[1] ?? "").includes(":after_seq:"),
    ) ||
      afterSeqBounds(event) !== undefined)
  );
}

/**
 * Display order: by order number among events that carry one, by time
 * otherwise. Seq-bearing events keep the slots they hold in time order and are
 * re-sorted among themselves, so the result is a total order even when only
 * some events carry a number (relay pages do not, live frames do).
 */
export function orderBySeqWithinTime(
  eventsByTimeAscending: RelayEvent[],
): RelayEvent[] {
  const slots: number[] = [];
  const numbered: RelayEvent[] = [];
  eventsByTimeAscending.forEach((event, index) => {
    if (event.seq !== undefined) {
      slots.push(index);
      numbered.push(event);
    }
  });
  if (numbered.length < 2) return eventsByTimeAscending;
  const bySeq = [...numbered].sort((a, b) => (a.seq ?? 0) - (b.seq ?? 0));
  if (bySeq.every((event, index) => event === numbered[index]))
    return eventsByTimeAscending;
  const result = [...eventsByTimeAscending];
  slots.forEach((slot, index) => {
    result[slot] = bySeq[index];
  });
  return result;
}
