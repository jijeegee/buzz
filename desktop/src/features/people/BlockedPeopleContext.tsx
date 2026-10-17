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

export type BlockedPeople = {
  blockedIds: ReadonlySet<string>;
  isBlocked: (pubkey: string | null | undefined) => boolean;
};

const EMPTY_IDS: ReadonlySet<string> = new Set();

/** Nothing blocked: outside the app shell, or before the list loads. */
const NONE_BLOCKED: BlockedPeople = {
  blockedIds: EMPTY_IDS,
  isBlocked: () => false,
};

export const blockedPeopleQueryKey = (pubkey: string) =>
  ["blocked-people", pubkey.toLowerCase()] as const;

const BLOCKED_PEOPLE_ROOT = ["blocked-people"] as const;

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

const BlockedPeopleContext = React.createContext<BlockedPeople>(NONE_BLOCKED);

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
  const value = React.useMemo<BlockedPeople>(
    () =>
      blockedIds.size === 0
        ? NONE_BLOCKED
        : {
            blockedIds,
            isBlocked: (pubkey) =>
              Boolean(pubkey) && blockedIds.has(pubkey?.toLowerCase() ?? ""),
          },
    [blockedIds],
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

function setCachedBlocked(
  queryClient: QueryClient,
  pubkey: string,
  blocked: boolean,
) {
  const target = pubkey.toLowerCase();
  queryClient.setQueriesData<BlockedPerson[]>(
    { queryKey: BLOCKED_PEOPLE_ROOT },
    (current) => {
      const rest = (current ?? []).filter(
        (person) => person.pubkey.toLowerCase() !== target,
      );
      return blocked ? [{ pubkey: target, createdAt: null }, ...rest] : rest;
    },
  );
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
      setCachedBlocked(queryClient, target, true);
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
      void queryClient.invalidateQueries({ queryKey: BLOCKED_PEOPLE_ROOT });
      if (currentPubkey) {
        void queryClient.invalidateQueries({
          queryKey: contactListQueryKey(currentPubkey),
        });
      }
    },
  });
}

/** Unblock one person. They do not come back as a friend. */
export function useUnblockPersonMutation() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (pubkey: string) => {
      await setPersonBlocked(pubkey, false);
      setCachedBlocked(queryClient, pubkey, false);
    },
    onSettled: () =>
      queryClient.invalidateQueries({ queryKey: BLOCKED_PEOPLE_ROOT }),
  });
}
