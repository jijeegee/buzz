import * as React from "react";

import {
  useContactListQuery,
  useFollowMutation,
  useUnfollowMutation,
} from "@/features/profile/hooks";

export type PeopleFriends = {
  friendIds: ReadonlySet<string>;
  isLoading: boolean;
  isFriend: (pubkey: string) => boolean;
  addFriend: (pubkey: string) => Promise<unknown>;
  removeFriend: (pubkey: string) => Promise<unknown>;
};

const EMPTY: ReadonlySet<string> = new Set();

/**
 * Friends are a one-way add, kept in my follow list (the same list the
 * profile's Follow button edits). No request or acceptance. Friends are
 * only ever added explicitly; nobody is added automatically.
 */
export function usePeopleFriends(
  currentPubkey: string | undefined,
): PeopleFriends {
  const contactsQuery = useContactListQuery(currentPubkey);
  const follow = useFollowMutation(currentPubkey);
  const unfollow = useUnfollowMutation(currentPubkey);
  const friendIds = React.useMemo(
    () =>
      contactsQuery.data
        ? new Set(
            contactsQuery.data.contacts.map((c) => c.pubkey.toLowerCase()),
          )
        : EMPTY,
    [contactsQuery.data],
  );
  const { mutateAsync: followAsync } = follow;
  const { mutateAsync: unfollowAsync } = unfollow;
  return React.useMemo(
    () => ({
      friendIds,
      isLoading: contactsQuery.isLoading,
      isFriend: (pubkey) => friendIds.has(pubkey.toLowerCase()),
      addFriend: (pubkey) => followAsync(pubkey.toLowerCase()),
      removeFriend: (pubkey) => unfollowAsync(pubkey.toLowerCase()),
    }),
    [contactsQuery.isLoading, followAsync, friendIds, unfollowAsync],
  );
}
