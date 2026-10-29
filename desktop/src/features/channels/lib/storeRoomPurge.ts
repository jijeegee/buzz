/**
 * Applies the membership list of a successful channel fetch to the device's
 * stored copy of rooms. Rules: docs/message-sync.md#forgetting-a-room.
 */
import { isJoinedChat } from "@/features/sidebar/lib/chatList";
import type { GetChannelsPayload } from "@/shared/api/tauriChannels";
import { invokeTauri } from "@/shared/api/tauri";
import type { Channel } from "@/shared/api/types";

type PurgeDeps = {
  /** Refused by the store when `scope` is no longer current. */
  forgetExcept: (roomIds: string[], scope: string) => Promise<void>;
};

const defaultDeps: PurgeDeps = {
  forgetExcept: (roomIds, scope) =>
    invokeTauri<void>("message_store_forget_rooms_except", { roomIds, scope }),
};

let deps = defaultDeps;
let started = 0;
let applied = 0;

/** Replace the store bridge (tests). Returns the restore function. */
export function setStoreRoomPurgeDeps(next: Partial<PurgeDeps>) {
  const previous = deps;
  deps = { ...deps, ...next };
  return () => {
    deps = previous;
  };
}

/**
 * Call when a channel fetch starts. The returned function receives the
 * fetch's payload once it succeeded (never for a failed fetch), with the
 * channel lists showing what is joined at settlement. Only a list newer than
 * the last one acted on may forget rooms, under the store capture the backend
 * took before that fetch for the same identity. The rooms kept are the
 * fetch's membership (hidden DMs included) plus every room any of `joinedNow`
 * shows as joined, so a room joined or created during the fetch stays. An
 * empty membership answer forgets nothing, whatever is shown joined.
 */
export function beginStoreRoomList(): (
  payload: Pick<GetChannelsPayload, "storeScope" | "memberIds">,
  joinedNow: ReadonlyArray<Channel[] | undefined>,
) => void {
  const seq = ++started;
  return ({ storeScope, memberIds }, joinedNow) => {
    if (!storeScope || memberIds.length === 0 || seq <= applied) return;
    const keep = new Set(memberIds);
    for (const channels of joinedNow) {
      for (const channel of channels ?? []) {
        if (isJoinedChat(channel)) keep.add(channel.id);
      }
    }
    applied = seq;
    deps.forgetExcept([...keep], storeScope).catch((error: unknown) => {
      console.warn("[MessageStore] forgetting rooms failed", error);
    });
  };
}
