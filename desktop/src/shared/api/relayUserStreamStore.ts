/**
 * The device store calls the user stream makes (`message_store_*`,
 * `commands/message_store_stream.rs`): the `room_seq` watermark, its epoch
 * and the stream's `sync_state` values (docs/message-sync.md#user-stream).
 */
import { getChannelReconnectRepairEvents } from "@/shared/api/channelReconnectRepair";
import type { RoomLiveBundle } from "@/shared/api/relayRoomBundle";
import type { UserStreamSession } from "@/shared/api/relayUserStream";
import {
  type StoredSeqs,
  type StreamStore,
  UserStreamLive,
} from "@/shared/api/relayUserStreamLive";
import { invokeTauri } from "@/shared/api/tauri";

export const tauriStreamStore: StreamStore = {
  epoch: (epoch) =>
    invokeTauri<boolean>("message_store_stream_epoch", { epoch }),
  seqs: async (roomIds, before) => {
    const stored = await invokeTauri<StoredSeqs | null>(
      "message_store_stream_seqs",
      { roomIds, before: before ?? null },
    );
    return stored ?? { vouched: false, epoch: null, rows: {}, resume: {} };
  },
  clearRows: async (roomIds) => {
    await invokeTauri<boolean>("message_store_clear_stream_rows", { roomIds });
  },
  scope: (subId) =>
    invokeTauri<string | null>("message_store_scope", { subId: subId ?? null }),
  resumePoints: async (roomIds, before) =>
    (await invokeTauri<Record<string, number> | null>(
      "message_store_resume_points",
      { roomIds, before },
    )) ?? {},
  segments: (roomIds) =>
    invokeTauri<Record<string, [number, number][]> | null>(
      "message_store_walk_baseline",
      { roomIds },
    ),
  freshIds: (ids, sinceMs) =>
    invokeTauri<string[] | null>("message_store_fresh_ids", { ids, sinceMs }),
  extend: (roomIds, from, to, scope, seq) =>
    seq
      ? invokeTauri<void>("message_store_extend_stream_coverage", {
          roomIds,
          from,
          to,
          scope,
          epoch: seq.epoch,
          seqs: seq.seqs,
        })
      : invokeTauri<void>("message_store_extend_coverage", {
          roomIds,
          from,
          to,
          scope,
        }),
  getState: (key) =>
    invokeTauri<string | null>("message_store_stream_state", { key }),
  setState: async (key, value) => {
    await invokeTauri<boolean>("message_store_set_stream_state", {
      key,
      value,
    });
  },
};

/** Lets `userStream` run live/gated over `bundle` (one per session). */
export function attachUserStreamLive(
  userStream: UserStreamSession,
  bundle: RoomLiveBundle,
  listRefresh: () => void,
  visibleRoom: () => string | null,
) {
  const live = new UserStreamLive({
    nowSecs: () => Math.floor(Date.now() / 1000),
    store: tauriStreamStore,
    bundle,
    roomOf: (event) => bundle.streamRoomOf(event),
    requestPage: getChannelReconnectRepairEvents,
    report: () => userStream.report,
    listRefresh,
    log: (line) => console.info(line),
    visibleRoom,
  });
  userStream.liveHooks = live;
  return live;
}
