import * as React from "react";
import { useQueryClient } from "@tanstack/react-query";

import { useChannelsQuery } from "@/features/channels/hooks";
import {
  contactListQueryKey,
  useContactListQuery,
  useFollowMutation,
  useUnfollowMutation,
  useUsersBatchQuery,
} from "@/features/profile/hooks";
import { getContactList, setContactList } from "@/shared/api/social";

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
 * profile's Follow button edits). No request or acceptance.
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

const seededKey = (pubkey: string) =>
  `kuzz-friends-seeded-from-dms.v1:${pubkey.toLowerCase()}`;

/** The people (not agents) I already have a one-to-one or group DM with. */
export function dmPeople({
  channels,
  currentPubkey,
  profiles,
}: {
  channels: readonly { channelType: string; participantPubkeys: string[] }[];
  currentPubkey: string;
  profiles: Readonly<Record<string, { isAgent?: boolean }>>;
}): string[] {
  const self = currentPubkey.toLowerCase();
  const people = new Set<string>();
  for (const channel of channels) {
    if (channel.channelType !== "dm") continue;
    for (const raw of channel.participantPubkeys) {
      const pubkey = raw.toLowerCase();
      const profile = profiles[pubkey];
      // Unknown profiles are skipped: they may be agents.
      if (pubkey !== self && profile && !profile.isAgent) people.add(pubkey);
    }
  }
  return [...people];
}

/**
 * Once per account on this device, adds everyone I already DM with as a
 * friend, so the list is not empty on day one. Later removals stick.
 */
export function useSeedFriendsFromDms(currentPubkey: string | undefined) {
  const queryClient = useQueryClient();
  const channelsQuery = useChannelsQuery();
  const alreadySeeded =
    !currentPubkey || localStorage.getItem(seededKey(currentPubkey)) === "1";
  const participants = React.useMemo(() => {
    const set = new Set<string>();
    for (const channel of channelsQuery.data ?? []) {
      if (channel.channelType !== "dm") continue;
      for (const pubkey of channel.participantPubkeys) {
        set.add(pubkey.toLowerCase());
      }
    }
    return [...set];
  }, [channelsQuery.data]);
  const profilesQuery = useUsersBatchQuery(participants, {
    enabled: !alreadySeeded && participants.length > 0,
  });
  const started = React.useRef(false);
  React.useEffect(() => {
    if (alreadySeeded || started.current || !currentPubkey) return;
    if (!channelsQuery.data) return;
    if (participants.length > 0 && !profilesQuery.data) return;
    started.current = true;
    const people = dmPeople({
      channels: channelsQuery.data,
      currentPubkey,
      profiles: profilesQuery.data?.profiles ?? {},
    });
    void (async () => {
      try {
        if (people.length > 0) {
          // Re-read right before writing: the list is replaced as a whole.
          const current = await getContactList(currentPubkey);
          const have = new Set(
            current.contacts.map((c) => c.pubkey.toLowerCase()),
          );
          const added = people.filter((pubkey) => !have.has(pubkey));
          if (added.length > 0) {
            await setContactList([
              ...current.contacts,
              ...added.map((pubkey) => ({ pubkey })),
            ]);
            await queryClient.invalidateQueries({
              queryKey: contactListQueryKey(currentPubkey),
            });
          }
        }
        localStorage.setItem(seededKey(currentPubkey), "1");
      } catch {
        // Try again next time People opens.
        started.current = false;
      }
    })();
  }, [
    alreadySeeded,
    channelsQuery.data,
    currentPubkey,
    participants.length,
    profilesQuery.data,
    queryClient,
  ]);
}
