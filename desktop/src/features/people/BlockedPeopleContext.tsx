import * as React from "react";
import {
  type QueryClient,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";

import { editContactList } from "@/features/people/lib/contactListEdit";
import { contactListQueryKey } from "@/features/profile/hooks";
import { useIdentityQuery } from "@/shared/api/hooks";
import {
  type BlockedPerson,
  listBlockedPeople,
  setPersonBlocked,
} from "@/shared/api/people";
import { getContactList, setContactList } from "@/shared/api/social";
import { SHARED_PEOPLE_ROOT } from "./useSharedPeopleSearch";

export type BlockedPeople = {
  blockedIds: ReadonlySet<string>;
  isBlocked: (pubkey: string | null | undefined) => boolean;
  /**
   * The list has loaded. Until then (or when it failed) nobody reads as
   * blocked, so anything that writes based on it must wait for this.
   */
  ready: boolean;
};

const EMPTY_IDS: ReadonlySet<string> = new Set();

/** Not known yet: outside the app shell, or before the list loads. */
const NOT_LOADED: BlockedPeople = {
  blockedIds: EMPTY_IDS,
  isBlocked: () => false,
  ready: false,
};

const NONE_BLOCKED: BlockedPeople = { ...NOT_LOADED, ready: true };

export const blockedPeopleQueryKey = (pubkey: string) =>
  ["blocked-people", pubkey.toLowerCase()] as const;

/** The people I blocked, newest first as the server sends them. */
export function useBlockedPeopleQuery(pubkey: string | undefined) {
  return useQuery<BlockedPerson[]>({
    enabled: Boolean(pubkey),
    queryKey: blockedPeopleQueryKey(pubkey ?? ""),
    queryFn: listBlockedPeople,
    staleTime: 60_000,
    gcTime: 10 * 60_000,
  });
}

export function blockedSet(
  people: readonly BlockedPerson[] | undefined,
): ReadonlySet<string> {
  if (!people || people.length === 0) return EMPTY_IDS;
  return new Set(people.map((person) => person.pubkey.toLowerCase()));
}

const BlockedPeopleContext = React.createContext<BlockedPeople>(NOT_LOADED);

/**
 * The signed-in account's blocked people for every screen: the People list,
 * the new message picker and channel timelines all read this one set.
 */
export function BlockedPeopleProvider({
  children,
}: {
  children: React.ReactNode;
}) {
  const identityQuery = useIdentityQuery();
  const query = useBlockedPeopleQuery(identityQuery.data?.pubkey);
  const blockedIds = React.useMemo(() => blockedSet(query.data), [query.data]);
  const ready = query.isSuccess;
  const value = React.useMemo<BlockedPeople>(
    () =>
      blockedIds.size === 0
        ? ready
          ? NONE_BLOCKED
          : NOT_LOADED
        : {
            blockedIds,
            isBlocked: (pubkey) =>
              Boolean(pubkey) && blockedIds.has(pubkey?.toLowerCase() ?? ""),
            ready,
          },
    [blockedIds, ready],
  );
  return (
    <BlockedPeopleContext.Provider value={value}>
      {children}
    </BlockedPeopleContext.Provider>
  );
}

export function useBlockedPeople(): BlockedPeople {
  return React.useContext(BlockedPeopleContext);
}

/** Update only `owner`'s cached block list; other accounts keep theirs. */
function setCachedBlocked(
  queryClient: QueryClient,
  owner: string,
  pubkey: string,
  blocked: boolean,
) {
  const target = pubkey.toLowerCase();
  queryClient.setQueryData<BlockedPerson[]>(
    blockedPeopleQueryKey(owner),
    (current) => {
      const rest = (current ?? []).filter(
        (person) => person.pubkey.toLowerCase() !== target,
      );
      return blocked ? [{ pubkey: target, createdAt: null }, ...rest] : rest;
    },
  );
}

/** After a block change: refetch this account's list and channel people. */
function refreshAfterBlockChange(queryClient: QueryClient, owner: string) {
  void queryClient.invalidateQueries({
    queryKey: blockedPeopleQueryKey(owner),
  });
  void queryClient.invalidateQueries({ queryKey: SHARED_PEOPLE_ROOT });
}

/** Thrown when the block went through but the friend removal did not. */
export const BLOCKED_BUT_STILL_FRIEND =
  "Blocked. Couldn't remove them from your friends. Try removing them again.";

/**
 * Block one person: the server stops delivering their messages to me, and
 * they leave my friends (one remove on the latest list).
 */
export function useBlockPersonMutation(currentPubkey: string | undefined) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (pubkey: string) => {
      if (!currentPubkey) throw new Error("No identity");
      const target = pubkey.toLowerCase();
      await setPersonBlocked(target, true);
      setCachedBlocked(queryClient, currentPubkey, target, true);
      try {
        await editContactList(
          currentPubkey,
          { type: "remove", pubkey: target },
          { read: getContactList, write: setContactList },
        );
      } catch {
        throw new Error(BLOCKED_BUT_STILL_FRIEND);
      }
    },
    onSettled: () => {
      if (!currentPubkey) return;
      refreshAfterBlockChange(queryClient, currentPubkey);
      void queryClient.invalidateQueries({
        queryKey: contactListQueryKey(currentPubkey),
      });
    },
  });
}

/** Unblock one person. They do not come back as a friend. */
export function useUnblockPersonMutation(currentPubkey: string | undefined) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (pubkey: string) => {
      if (!currentPubkey) throw new Error("No identity");
      await setPersonBlocked(pubkey, false);
      setCachedBlocked(queryClient, currentPubkey, pubkey, false);
    },
    onSettled: () => {
      if (currentPubkey) refreshAfterBlockChange(queryClient, currentPubkey);
    },
  });
}
