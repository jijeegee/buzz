/**
 * Refreshes room lists and rosters when the open room's timeline reports a
 * membership change (someone joined, accepted an invitation, left or was
 * removed).
 *
 * The relay publishes the `member_joined` / `member_left` system message
 * before it rebuilds the room's kind:39002 member list, and both the room
 * list (`memberPubkeys`, which drives a chat's title and grid avatar) and the
 * roster (Chat info) are read from that list. A refetch fired the moment the
 * system message arrives can therefore still read the old member list and
 * settle on it. So the refresh runs immediately and again once the relay has
 * had time to publish the new list.
 */

import type { useQueryClient } from "@tanstack/react-query";

import { noteChannelMembershipChange } from "@/shared/api/channelMembershipWrites";

const MEMBERSHIP_SYSTEM_TYPES = new Set([
  "member_joined",
  "member_left",
  "member_removed",
  "admin_kick",
]);

/** Delays (ms) of the follow-up refreshes after the immediate one. */
export const MEMBERSHIP_SETTLE_DELAYS_MS = [1_000, 4_000] as const;

type RefreshDeps = {
  queryClient: Pick<ReturnType<typeof useQueryClient>, "invalidateQueries">;
  noteMembershipChange?: (channelId: string) => void;
  setTimer?: (callback: () => void, delayMs: number) => unknown;
};

function refreshNow(
  channelId: string,
  { queryClient, noteMembershipChange }: RefreshDeps,
) {
  (noteMembershipChange ?? noteChannelMembershipChange)(channelId);
  void queryClient.invalidateQueries({
    queryKey: ["channels"],
    exact: true,
  });
}

/**
 * Handles a live system message for `channelId`. Returns true when it was a
 * membership change (and refreshes were scheduled).
 */
export function refreshOnSystemMessage(
  channelId: string,
  content: string,
  deps: RefreshDeps,
): boolean {
  let type: string | undefined;
  try {
    type = (JSON.parse(content) as { type?: string }).type;
  } catch {
    return false; // Non-JSON system message — ignore.
  }
  if (type === "picture_changed") {
    // Group photo changed: refresh the list so every avatar updates.
    void deps.queryClient.invalidateQueries({
      queryKey: ["channels"],
      exact: true,
    });
    return false;
  }
  if (!type || !MEMBERSHIP_SYSTEM_TYPES.has(type)) return false;

  refreshNow(channelId, deps);
  const setTimer = deps.setTimer ?? ((cb, ms) => setTimeout(cb, ms));
  for (const delayMs of MEMBERSHIP_SETTLE_DELAYS_MS) {
    setTimer(() => refreshNow(channelId, deps), delayMs);
  }
  return true;
}

/**
 * Follow-up timers owned by one open room: `clear()` drops the pending ones
 * when the room closes, so a closed room is not refreshed later.
 */
export function createSettleTimers() {
  const pending = new Set<ReturnType<typeof setTimeout>>();
  return {
    setTimer(callback: () => void, delayMs: number) {
      const handle = setTimeout(() => {
        pending.delete(handle);
        callback();
      }, delayMs);
      pending.add(handle);
    },
    clear() {
      for (const handle of pending) clearTimeout(handle);
      pending.clear();
    },
  };
}
