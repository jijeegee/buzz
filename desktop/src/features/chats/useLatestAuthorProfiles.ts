import { useMemo } from "react";

import { useUsersBatchQuery } from "@/features/profile/hooks";
import type { UserProfileLookup } from "@/features/profile/lib/identity";
import type { ChatListRow } from "./chatListState";

/** Profiles of the people who wrote each chat's latest message. */
export function useLatestAuthorProfiles(
  rows: readonly ChatListRow[] | undefined,
): UserProfileLookup | undefined {
  const authors = useMemo(() => {
    const set = new Set<string>();
    for (const row of rows ?? []) {
      if (row.latest) set.add(row.latest.pubkey.toLowerCase());
    }
    return [...set];
  }, [rows]);
  const query = useUsersBatchQuery(authors, { enabled: rows !== undefined });
  return query.data?.profiles;
}
