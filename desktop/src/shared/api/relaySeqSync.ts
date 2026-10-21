import {
  getChannelReconnectRepairEvents,
  getChannelSeqRepairEvents,
} from "@/shared/api/channelReconnectRepair";
import type {
  RelaySubscription,
  RelaySubscriptionFilter,
} from "@/shared/api/relayClientShared";
import {
  replayLiveSubscriptions,
  type SeqReplay,
} from "@/shared/api/relayReconnectReplay";
import {
  probeKuzzSeq,
  type KuzzSeqProbe,
} from "@/shared/api/relaySeqCapability";
import {
  frameChannelSequence,
  isAfterSeqBoundsEvent,
  SeqCursors,
  withKuzzSeqParam,
} from "@/shared/api/relaySeqCursor";
import type { RelayEvent } from "@/shared/api/types";

/** The channel a live event belongs to: the filter's one `#h`, else the event's `h`. */
function channelIdOf(filter: RelaySubscriptionFilter, event: RelayEvent) {
  if (filter["#h"]?.length === 1) return filter["#h"][0];
  return event.tags.find((tag) => tag[0] === "h")?.[1];
}

/**
 * The relay session's `kuzz-seq` state: whether the current connection opted
 * in (`?kuzz_seq=1`, only when NIP-11 advertises it), the channel order
 * numbers seen this session (kept across reconnects, dropped on account or
 * community switch), and order-number repair on an open socket.
 */
type RelaySeqHost = {
  generation: number;
  subscriptions: Map<string, RelaySubscription>;
  sendRaw: (payload: unknown[]) => Promise<void>;
  visibleChannelId: string | null;
};

export class RelaySeqSync {
  /** Connection generation opted in to `kuzz-seq`, if the current one is. */
  private generation: number | null = null;
  readonly cursors = new SeqCursors();
  private resyncRunning = false;
  private resyncAgain = false;
  /** Injectable for tests. */
  probe: KuzzSeqProbe = probeKuzzSeq;

  private readonly host: () => RelaySeqHost;

  constructor(host: () => RelaySeqHost) {
    this.host = host;
  }

  /** The socket address for `relayUrl`; AUTH keeps signing `relayUrl`. */
  async socketUrl(relayUrl: string) {
    return (await this.probe(relayUrl)) ? withKuzzSeqParam(relayUrl) : relayUrl;
  }

  connected(generation: number, socketUrl: string, relayUrl: string) {
    this.generation = socketUrl === relayUrl ? null : generation;
  }

  /** The socket went away; numbers stay for the next connection's repair. */
  reset() {
    this.generation = null;
  }

  clear() {
    this.generation = null;
    this.cursors.clear();
  }

  isOptedIn(generation: number) {
    return this.generation === generation;
  }

  /** Tag an EVENT frame's number; false for a frame that is no channel event. */
  onEvent(
    subscription: RelaySubscription,
    event: RelayEvent,
    generation: number,
    meta: unknown,
  ) {
    // An after_seq page's bounds shares kind 39006 with window bounds.
    if (isAfterSeqBoundsEvent(event)) return false;
    const seq = this.isOptedIn(generation)
      ? frameChannelSequence(meta)
      : undefined;
    if (seq === undefined) return true;
    event.seq = seq;
    // Only live numbers move the cursor: an older history page would drag
    // the replay point back.
    if (subscription.mode === "live") {
      const channelId = channelIdOf(subscription.filter, event);
      if (channelId) this.cursors.observe(channelId, seq);
    }
    return true;
  }

  /** Tag an accepted publish with the number in its OK frame. */
  onOk(
    pending: { event: RelayEvent } | undefined,
    generation: number,
    meta: unknown,
  ) {
    const seq = this.isOptedIn(generation)
      ? frameChannelSequence(meta)
      : undefined;
    if (!pending || seq === undefined) return;
    const event = pending.event;
    event.seq = seq;
    const channelId = event.tags.find((tag) => tag[0] === "h")?.[1];
    if (channelId) this.cursors.observe(channelId, seq);
  }

  replay(generation: number): SeqReplay | undefined {
    return this.isOptedIn(generation)
      ? { cursors: this.cursors, requestSeqRepair: getChannelSeqRepairEvents }
      : undefined;
  }

  /** No live subscription is open on the filter's channel, but a number is known. */
  isFirstForKnownChannel(
    subscriptions: Map<string, RelaySubscription>,
    filter: RelaySubscriptionFilter,
  ) {
    const channelId = filter["#h"]?.length === 1 ? filter["#h"][0] : undefined;
    if (!channelId || this.cursors.get(channelId) === undefined) return false;
    return ![...subscriptions.values()].some(
      (subscription) =>
        subscription.mode === "live" &&
        subscription.filter["#h"]?.includes(channelId),
    );
  }

  /**
   * Order-number repair on the current socket (live REQs stay open): every
   * channel after `KUZZ_RESYNC`, or just `onlySubIds`. Resync passes never
   * overlap; one that arrives mid-pass reruns once after it.
   */
  async repair(onlySubIds?: Set<string>) {
    const { generation, subscriptions, sendRaw, visibleChannelId } =
      this.host();
    const currentGeneration = () => this.host().generation;
    const seq = this.replay(generation);
    if (!seq) return;
    if (!onlySubIds) {
      if (this.resyncRunning) {
        this.resyncAgain = true;
        return;
      }
      this.resyncRunning = true;
    }
    try {
      do {
        if (!onlySubIds) this.resyncAgain = false;
        await replayLiveSubscriptions({
          subscriptions,
          sendRaw,
          requestRepair: getChannelReconnectRepairEvents,
          generation,
          visibleChannelId,
          isActive: () => currentGeneration() === generation,
          seq,
          resendLive: false,
          onlySubIds,
        });
      } while (
        !onlySubIds &&
        this.resyncAgain &&
        currentGeneration() === generation
      );
    } catch (error) {
      // Best-effort: the next reconnect or resync retries from the pin.
      console.warn("[relay] order-number repair failed:", error);
    } finally {
      if (!onlySubIds) this.resyncRunning = false;
    }
  }
}
