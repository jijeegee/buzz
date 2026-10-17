import * as React from "react";
import { useQuery } from "@tanstack/react-query";

import { personToSearchResult, searchSharedPeople } from "@/shared/api/people";
import type { UserSearchResult } from "@/shared/api/types";

/** Wait this long after the last keystroke before asking the server. */
export const SHARED_PEOPLE_DEBOUNCE_MS = 250;
const SHARED_PEOPLE_LIMIT = 50;
const NO_PEOPLE: UserSearchResult[] = [];

/** `value`, once it has stopped changing for `delayMs`. */
export function useDebouncedValue<T>(value: T, delayMs: number): T {
  const [settled, setSettled] = React.useState(value);
  React.useEffect(() => {
    const timer = setTimeout(() => setSettled(value), delayMs);
    return () => clearTimeout(timer);
  }, [delayMs, value]);
  return settled;
}

/**
 * "In your channels": people in channels I am in whose name starts with
 * `query` (server-side, debounced). Idle while the query is empty, so nothing
 * loads the whole directory.
 */
export function useSharedPeopleSearch(
  query: string,
  { enabled = true }: { enabled?: boolean } = {},
) {
  const needle = useDebouncedValue(
    query.trim().toLowerCase(),
    SHARED_PEOPLE_DEBOUNCE_MS,
  );
  const active = enabled && needle.length > 0;
  const result = useQuery({
    enabled: active,
    queryKey: ["people-shared", needle],
    queryFn: async () =>
      (await searchSharedPeople(needle, SHARED_PEOPLE_LIMIT)).map(
        personToSearchResult,
      ),
    staleTime: 30_000,
    gcTime: 5 * 60_000,
    retry: false,
  });
  const settling = active && needle !== query.trim().toLowerCase();
  return {
    people: active ? (result.data ?? NO_PEOPLE) : NO_PEOPLE,
    isLoading: settling || (active && result.isLoading),
    error: active && result.error instanceof Error ? result.error : null,
  };
}
