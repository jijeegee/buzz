import { invokeTauri } from "@/shared/api/tauri";
import type { RelayEvent } from "@/shared/api/types";

export type ChannelReconnectRepairRequest = {
  channelId: string;
  since: number;
  limit: number;
  until?: number;
  beforeId?: string;
};

export type ChannelSeqRepairRequest = {
  channelId: string;
  afterSeq: number;
  limit: number;
};

/** Fetch one fixed-kind, channel-scoped reconnect repair page. */
export async function getChannelReconnectRepairEvents({
  channelId,
  since,
  limit,
  until,
  beforeId,
}: ChannelReconnectRepairRequest): Promise<RelayEvent[]> {
  return invokeTauri<RelayEvent[]>("get_channel_reconnect_repair", {
    channelId,
    since,
    afterSeq: null,
    limit,
    until: until ?? null,
    beforeId: beforeId ?? null,
  });
}

/**
 * Fetch one `after_seq` repair page from a `kuzz-seq` relay: the channel's
 * events after `afterSeq` in order-number order, then one 39006 bounds event.
 */
export async function getChannelSeqRepairEvents({
  channelId,
  afterSeq,
  limit,
}: ChannelSeqRepairRequest): Promise<RelayEvent[]> {
  return invokeTauri<RelayEvent[]>("get_channel_reconnect_repair", {
    channelId,
    since: null,
    afterSeq,
    limit,
    until: null,
    beforeId: null,
  });
}
