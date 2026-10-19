import * as React from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";

import { useDebouncedValue } from "@/features/people/useSharedPeopleSearch";
import { useIdentityQuery } from "@/shared/api/hooks";
import {
  GROUP_SEARCH_DEBOUNCE_MS,
  GROUP_SEARCH_MIN_LENGTH,
  type GroupSummary,
  getGroup,
  getGroupAddPolicy,
  getGroupInviteLink,
  listGroupInvitations,
  searchGroups,
} from "./groupsApi";

const NO_GROUPS: GroupSummary[] = [];

/** Every groups query for the signed-in account (scoped by pubkey). */
export const GROUPS_QUERY_ROOT = ["groups-api"] as const;

function useAccountKey(): string {
  return useIdentityQuery().data?.pubkey?.toLowerCase() ?? "";
}

export function groupDetailQueryKey(account: string, groupId: string) {
  return [...GROUPS_QUERY_ROOT, account, "detail", groupId] as const;
}

export function groupInviteLinkQueryKey(account: string, groupId: string) {
  return [...GROUPS_QUERY_ROOT, account, "invite-link", groupId] as const;
}

export function groupInvitationsQueryKey(account: string) {
  return [...GROUPS_QUERY_ROOT, account, "invitations"] as const;
}

export function groupAddPolicyQueryKey(account: string) {
  return [...GROUPS_QUERY_ROOT, account, "add-policy"] as const;
}

/**
 * Find groups: public groups from `GET /api/groups/search`, debounced 250 ms
 * and idle below 2 characters (the server never lists every group).
 */
export function useGroupSearch(query: string, enabled: boolean) {
  const trimmed = query.trim();
  const needle = useDebouncedValue(trimmed, GROUP_SEARCH_DEBOUNCE_MS);
  const account = useAccountKey();
  const active =
    enabled && Boolean(account) && needle.length >= GROUP_SEARCH_MIN_LENGTH;
  const result = useQuery({
    enabled: active,
    queryKey: [...GROUPS_QUERY_ROOT, account, "search", needle.toLowerCase()],
    queryFn: () => searchGroups(needle),
    staleTime: 30_000,
    retry: false,
  });
  const settling =
    enabled && trimmed.length >= GROUP_SEARCH_MIN_LENGTH && needle !== trimmed;
  return {
    groups: active ? (result.data ?? NO_GROUPS) : NO_GROUPS,
    isLoading: settling || (active && result.isLoading),
    error: active && result.error instanceof Error ? result.error : null,
  };
}

/** `GET /api/groups/{id}`: handle, visibility and invite policy. */
export function useGroupDetailQuery(groupId: string | null, enabled = true) {
  const account = useAccountKey();
  return useQuery({
    enabled: enabled && Boolean(account) && groupId !== null,
    queryKey: groupDetailQueryKey(account, groupId ?? ""),
    queryFn: () => getGroup(groupId ?? ""),
    staleTime: 30_000,
    retry: false,
  });
}

/** `GET /api/groups/{id}/invite-link` (created on first call). */
export function useGroupInviteLinkQuery(
  groupId: string | null,
  enabled: boolean,
) {
  const account = useAccountKey();
  return useQuery({
    enabled: enabled && Boolean(account) && groupId !== null,
    queryKey: groupInviteLinkQueryKey(account, groupId ?? ""),
    queryFn: () => getGroupInviteLink(groupId ?? ""),
    staleTime: 5 * 60_000,
    retry: false,
  });
}

export function useGroupAddPolicyQuery() {
  const account = useAccountKey();
  return useQuery({
    enabled: Boolean(account),
    queryKey: groupAddPolicyQueryKey(account),
    queryFn: getGroupAddPolicy,
    staleTime: 60_000,
    retry: false,
  });
}

/**
 * Pending group invitations. There is no push yet, so the list refreshes on
 * app start (mount), whenever the window regains focus, and when the caller
 * bumps `refreshKey` (Chats opened).
 */
export function useGroupInvitationsQuery(refreshKey: unknown) {
  const account = useAccountKey();
  const queryClient = useQueryClient();
  const queryKey = groupInvitationsQueryKey(account);
  const query = useQuery({
    enabled: Boolean(account),
    queryKey,
    queryFn: listGroupInvitations,
    staleTime: 15_000,
    retry: false,
  });

  const keyString = JSON.stringify(queryKey);
  React.useEffect(() => {
    if (!account) return;
    const refresh = () =>
      void queryClient.invalidateQueries({ queryKey: JSON.parse(keyString) });
    window.addEventListener("focus", refresh);
    return () => window.removeEventListener("focus", refresh);
  }, [account, keyString, queryClient]);

  const firstRefreshKey = React.useRef(refreshKey);
  React.useEffect(() => {
    if (Object.is(firstRefreshKey.current, refreshKey)) return;
    firstRefreshKey.current = refreshKey;
    void queryClient.invalidateQueries({ queryKey: JSON.parse(keyString) });
  }, [keyString, queryClient, refreshKey]);

  return query;
}
