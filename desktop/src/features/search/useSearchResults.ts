import * as React from "react";

import {
  useManagedAgentsQuery,
  useRelayAgentsQuery,
} from "@/features/agents/hooks";
import { useIsArchivedPredicate } from "@/features/identity-archive/hooks";
import {
  useUserSearchQuery,
  useUsersBatchQuery,
} from "@/features/profile/hooks";
import { rankUserCandidatesBySearch } from "@/features/profile/lib/userCandidateSearch";
import { scoreChannelMatch } from "@/features/channels/lib/channelSearchScore";
import {
  mergeOpenChannelDirectory,
  useOpenChannelDirectoryQuery,
} from "@/features/channels/openChannelDirectory";
import {
  getMinimumSearchQueryLength,
  MIN_SEARCH_QUERY_LENGTH,
  useSearchMessagesQuery,
  useThreadNameSearchQuery,
} from "@/features/search/hooks";
import {
  compareGroupMatches,
  findThreadNameMatches,
  type GroupMatch,
  sortNewestFirst,
} from "@/features/search/lib/searchRanking";
import {
  isChannelUuid,
  isHexPubkey,
  normalizeFromHandle,
  normalizeInChannel,
  parseSearchOperators,
  type OperatorResolveResult,
} from "@/features/search/lib/parseSearchOperators";
import type { SearchResult } from "@/features/search/ui/SearchResultItem";
import { useSearchHitThreads } from "@/features/search/useSearchHitThreads";
import type { ChatListRow } from "@/features/chats/chatListState";
import type { ChatStateBinding } from "@/shared/api/chatState";
import type { Channel, SearchHit, UserSearchResult } from "@/shared/api/types";
import { normalizePubkey } from "@/shared/lib/pubkey";

const NO_HITS: SearchHit[] = [];

function formatUserResultName(user: UserSearchResult) {
  return user.displayName?.trim() || user.nip05Handle?.trim() || user.pubkey;
}

function dedupeSearchHits(hits: SearchHit[]) {
  const seenEventIds = new Set<string>();

  return hits.filter((hit) => {
    if (seenEventIds.has(hit.eventId)) {
      return false;
    }

    seenEventIds.add(hit.eventId);
    return true;
  });
}

function resolveChannelIdFromOperator(
  raw: string | null,
  channels: Channel[],
  channelLabels?: Record<string, string>,
): OperatorResolveResult<string> {
  if (!raw) {
    return { status: "none" };
  }
  const value = normalizeInChannel(raw);
  if (!value) {
    return { status: "none" };
  }
  if (isChannelUuid(value)) {
    return { status: "resolved", value };
  }
  const needle = value.toLowerCase();
  const match = channels.find((channel) => {
    const label = channelLabels?.[channel.id]?.trim() || channel.name;
    return (
      channel.name.toLowerCase() === needle || label.toLowerCase() === needle
    );
  });
  return match
    ? { status: "resolved", value: match.id }
    : { status: "unresolved" };
}

function resolveAuthorFromOperator(
  raw: string | null,
  candidates: Array<{ pubkey: string; displayName?: string | null }>,
): OperatorResolveResult<string> {
  if (!raw) {
    return { status: "none" };
  }
  if (isHexPubkey(raw)) {
    return { status: "resolved", value: normalizePubkey(raw) };
  }
  const handle = normalizeFromHandle(raw).toLowerCase();
  if (!handle) {
    return { status: "unresolved" };
  }
  const match = candidates.find((candidate) => {
    const name = candidate.displayName?.trim().toLowerCase();
    return name === handle || normalizePubkey(candidate.pubkey) === handle;
  });
  return match
    ? { status: "resolved", value: normalizePubkey(match.pubkey) }
    : { status: "unresolved" };
}

export function useSearchResults({
  channelLabels,
  channels: memberChannels,
  enabled,
  limit = 12,
  scopeChannelId,
  threadRows,
  threadSearchBinding,
}: {
  channelLabels?: Record<string, string>;
  channels: Channel[];
  enabled: boolean;
  limit?: number;
  scopeChannelId?: string | null;
  /** The relay's Chats list rows; their thread names are searched. */
  threadRows?: readonly ChatListRow[];
  /**
   * The account for the relay's thread-name search, which also finds closed
   * and quiet threads the Chats list leaves out. Null when the relay keeps
   * no chat state.
   */
  threadSearchBinding?: ChatStateBinding | null;
}) {
  const [query, setQuery] = React.useState("");
  const [debouncedQuery, setDebouncedQuery] = React.useState("");
  const [selectedIndex, setSelectedIndex] = React.useState(0);
  const isArchivedDiscovery = useIsArchivedPredicate();
  const parsedQuery = React.useMemo(
    () => parseSearchOperators(debouncedQuery),
    [debouncedQuery],
  );
  const minimumQueryLength = getMinimumSearchQueryLength(scopeChannelId);
  const hasSearchQuery =
    debouncedQuery.trim().length >= minimumQueryLength ||
    parsedQuery.since !== null ||
    parsedQuery.until !== null ||
    parsedQuery.from !== null ||
    parsedQuery.in !== null;
  const searchBackedQueriesEnabled = enabled && hasSearchQuery;

  // Global search surfaces non-member open channels, but only after a query
  // needs search-backed results. Opening Cmd-K with an empty query must keep
  // its suggestions local and avoid the all-open discovery scan. Scoped search
  // (channelId set) needs no directory.
  const openDirectoryQuery = useOpenChannelDirectoryQuery({
    enabled: searchBackedQueriesEnabled && !scopeChannelId,
  });
  const channels = React.useMemo(
    () => mergeOpenChannelDirectory(memberChannels, openDirectoryQuery.data),
    [memberChannels, openDirectoryQuery.data],
  );

  const channelLookup = React.useMemo(
    () => new Map(channels.map((channel) => [channel.id, channel])),
    [channels],
  );

  const channelResolution = React.useMemo<OperatorResolveResult<string>>(
    () =>
      scopeChannelId
        ? { status: "resolved", value: scopeChannelId }
        : resolveChannelIdFromOperator(parsedQuery.in, channels, channelLabels),
    [parsedQuery.in, channels, channelLabels, scopeChannelId],
  );

  const ftsQuery = parsedQuery.text;

  const needsAuthorResolution = Boolean(parsedQuery.from);
  const entitySearchEnabled = searchBackedQueriesEnabled && !scopeChannelId;

  const fromHandleForLookup =
    parsedQuery.from && !isHexPubkey(parsedQuery.from)
      ? normalizeFromHandle(parsedQuery.from)
      : "";

  const managedAgentsQuery = useManagedAgentsQuery({
    enabled:
      searchBackedQueriesEnabled && (!scopeChannelId || needsAuthorResolution),
  });
  const relayAgentsQuery = useRelayAgentsQuery({
    enabled:
      searchBackedQueriesEnabled && (!scopeChannelId || needsAuthorResolution),
  });
  // Resolve `from:@name` against people, not only agents.
  const fromUserSearchQuery = useUserSearchQuery(fromHandleForLookup, {
    enabled: searchBackedQueriesEnabled && fromHandleForLookup.length >= 1,
    limit,
  });
  const userSearchQuery = useUserSearchQuery(ftsQuery, {
    enabled: entitySearchEnabled,
    limit,
  });
  const fuzzyUserCandidatesQuery = useUserSearchQuery("", {
    allowEmpty: true,
    enabled: entitySearchEnabled && ftsQuery.length >= 4,
    limit: 100,
  });

  const authorCandidateSeed = React.useMemo(() => {
    const candidates: Array<{ pubkey: string; displayName?: string | null }> =
      [];
    const seen = new Set<string>();
    const push = (pubkey: string, displayName?: string | null) => {
      const key = normalizePubkey(pubkey);
      if (seen.has(key)) {
        return;
      }
      seen.add(key);
      candidates.push({ pubkey: key, displayName });
    };
    for (const agent of managedAgentsQuery.data ?? []) {
      push(agent.pubkey, agent.name);
    }
    for (const agent of relayAgentsQuery.data ?? []) {
      push(agent.pubkey, agent.name);
    }
    for (const user of fromUserSearchQuery.data ?? []) {
      push(user.pubkey, user.displayName);
    }
    for (const user of userSearchQuery.data ?? []) {
      push(user.pubkey, user.displayName);
    }
    return candidates;
  }, [
    managedAgentsQuery.data,
    relayAgentsQuery.data,
    fromUserSearchQuery.data,
    userSearchQuery.data,
  ]);

  const authorResolution = React.useMemo(
    () => resolveAuthorFromOperator(parsedQuery.from, authorCandidateSeed),
    [parsedQuery.from, authorCandidateSeed],
  );

  const hasUnresolvedOperator =
    authorResolution.status === "unresolved" ||
    channelResolution.status === "unresolved";

  // While `from:@name` user search is still loading, hold off so we do not
  // flash an unresolved empty state before candidates arrive.
  const waitingOnFromResolution =
    Boolean(fromHandleForLookup) &&
    fromUserSearchQuery.isLoading &&
    authorResolution.status === "unresolved";

  const searchQuery = useSearchMessagesQuery(ftsQuery, {
    enabled:
      enabled &&
      !hasUnresolvedOperator &&
      !waitingOnFromResolution &&
      ftsQuery.length >= minimumQueryLength,
    limit,
    channelId:
      channelResolution.status === "resolved"
        ? channelResolution.value
        : undefined,
    authors:
      authorResolution.status === "resolved"
        ? [authorResolution.value]
        : undefined,
    since: parsedQuery.since,
    until: parsedQuery.until,
    unresolvedOperator: hasUnresolvedOperator,
    minimumQueryLength,
  });

  const searchHits = React.useMemo(() => {
    if (hasUnresolvedOperator) {
      return [];
    }
    return sortNewestFirst(dedupeSearchHits(searchQuery.data?.hits ?? []));
  }, [hasUnresolvedOperator, searchQuery.data?.hits]);
  const hitThreads = useSearchHitThreads({
    channelLookup,
    enabled,
    hits: searchHits,
    threadRows,
  });
  // Held back until each hit's thread is known, so rows draw once.
  const messageResults = hitThreads.isLoading ? NO_HITS : searchHits;
  const channelMatches = React.useMemo(() => {
    if (scopeChannelId || ftsQuery.length < MIN_SEARCH_QUERY_LENGTH) {
      return [];
    }

    const normalizedQuery = ftsQuery.toLowerCase();

    return channels
      .flatMap((channel) => {
        const isVisible = channel.archivedAt
          ? channel.isMember
          : channel.visibility === "open" || channel.isMember;
        if (!isVisible) return [];

        const displayName = channelLabels?.[channel.id]?.trim() || channel.name;
        const displayScore = scoreChannelMatch(
          { name: displayName, description: channel.description },
          normalizedQuery,
        );
        const rawNameScore = scoreChannelMatch(
          { name: channel.name, description: "" },
          normalizedQuery,
        );
        const scores = [displayScore, rawNameScore].filter(
          (score): score is number => score !== null,
        );
        if (scores.length === 0) return [];

        const lastMessageAt = channel.lastMessageAt
          ? Date.parse(channel.lastMessageAt)
          : 0;
        const match: GroupMatch = {
          activity: Number.isFinite(lastMessageAt) ? lastMessageAt : 0,
          joined: channel.isMember || channel.channelType === "dm",
          label: displayName,
          score: Math.min(...scores),
        };
        return [{ channel, match }];
      })
      .sort((a, b) => compareGroupMatches(a.match, b.match))
      .slice(0, 5);
  }, [channelLabels, channels, ftsQuery, scopeChannelId]);
  const channelResults = React.useMemo(
    () => channelMatches.map(({ channel }) => channel),
    [channelMatches],
  );
  const threadNameSearch = useThreadNameSearchQuery(
    ftsQuery,
    threadSearchBinding,
    enabled && !scopeChannelId,
  );
  const searchedThreadRows = threadNameSearch.data;
  const threadMatches = React.useMemo(() => {
    if (
      scopeChannelId ||
      (!threadRows && !searchedThreadRows) ||
      ftsQuery.length < MIN_SEARCH_QUERY_LENGTH
    ) {
      return [];
    }
    // Chats rows first: a thread in both keeps its Chats row.
    return findThreadNameMatches(
      [...(threadRows ?? []), ...(searchedThreadRows ?? [])],
      ftsQuery.toLowerCase(),
      (channelId) => channelLookup.has(channelId),
    )
      .map(({ score, thread }) => ({
        isDm: channelLookup.get(thread.channelId)?.channelType === "dm",
        match: {
          activity: thread.at * 1_000,
          joined: true,
          label: thread.name,
          score,
        },
        thread,
      }))
      .sort((a, b) => compareGroupMatches(a.match, b.match))
      .slice(0, 5);
  }, [channelLookup, ftsQuery, scopeChannelId, searchedThreadRows, threadRows]);
  const managedAgentPubkeys = React.useMemo(
    () =>
      new Set(
        (managedAgentsQuery.data ?? []).map((agent) =>
          normalizePubkey(agent.pubkey),
        ),
      ),
    [managedAgentsQuery.data],
  );
  const relayAgentPubkeys = React.useMemo(
    () =>
      new Set(
        (relayAgentsQuery.data ?? []).map((agent) =>
          normalizePubkey(agent.pubkey),
        ),
      ),
    [relayAgentsQuery.data],
  );
  const eligibleAgentPubkeys = React.useMemo(() => {
    const pubkeys = new Set(managedAgentPubkeys);

    for (const agent of relayAgentsQuery.data ?? []) {
      if (agent.respondTo === "anyone") {
        pubkeys.add(normalizePubkey(agent.pubkey));
      }
    }

    return pubkeys;
  }, [managedAgentPubkeys, relayAgentsQuery.data]);
  const userResults = React.useMemo<UserSearchResult[]>(() => {
    if (scopeChannelId || ftsQuery.length < MIN_SEARCH_QUERY_LENGTH) {
      return [];
    }

    const candidatesByPubkey = new Map<string, UserSearchResult>();

    const addCandidate = (candidate: UserSearchResult) => {
      const pubkey = normalizePubkey(candidate.pubkey);

      if (isArchivedDiscovery(pubkey)) {
        return;
      }

      const isKnownAgent =
        candidate.isAgent ||
        managedAgentPubkeys.has(pubkey) ||
        relayAgentPubkeys.has(pubkey);

      if (isKnownAgent && !eligibleAgentPubkeys.has(pubkey)) {
        return;
      }

      const existing = candidatesByPubkey.get(pubkey);
      if (!existing) {
        candidatesByPubkey.set(pubkey, {
          ...candidate,
          pubkey,
          isAgent: isKnownAgent,
        });
        return;
      }

      candidatesByPubkey.set(pubkey, {
        pubkey,
        avatarUrl: existing.avatarUrl ?? candidate.avatarUrl ?? null,
        displayName:
          candidate.isAgent && candidate.displayName?.trim()
            ? candidate.displayName
            : (existing.displayName ?? candidate.displayName),
        nip05Handle: existing.nip05Handle ?? candidate.nip05Handle ?? null,
        ownerPubkey: existing.ownerPubkey ?? candidate.ownerPubkey ?? null,
        isAgent: existing.isAgent || isKnownAgent,
      });
    };

    for (const user of userSearchQuery.data ?? []) {
      addCandidate(user);
    }

    for (const user of fuzzyUserCandidatesQuery.data ?? []) {
      addCandidate(user);
    }

    for (const agent of relayAgentsQuery.data ?? []) {
      if (agent.respondTo !== "anyone") {
        continue;
      }

      const candidate = {
        pubkey: agent.pubkey,
        displayName: agent.name,
        avatarUrl: null,
        nip05Handle: null,
        ownerPubkey: agent.ownerPubkey,
        isAgent: true,
      };

      addCandidate(candidate);
    }

    for (const agent of managedAgentsQuery.data ?? []) {
      const candidate = {
        pubkey: agent.pubkey,
        displayName: agent.name,
        avatarUrl: null,
        nip05Handle: null,
        ownerPubkey: null,
        isAgent: true,
      };

      addCandidate(candidate);
    }

    return rankUserCandidatesBySearch({
      candidates: [...candidatesByPubkey.values()],
      getLabel: formatUserResultName,
      limit,
      query: ftsQuery,
    });
  }, [
    eligibleAgentPubkeys,
    fuzzyUserCandidatesQuery.data,
    ftsQuery,
    isArchivedDiscovery,
    limit,
    managedAgentPubkeys,
    managedAgentsQuery.data,
    relayAgentPubkeys,
    relayAgentsQuery.data,
    scopeChannelId,
    userSearchQuery.data,
  ]);

  const results = React.useMemo<SearchResult[]>(() => {
    // Group chats (group and thread names) first, then DMs, each ranked by
    // match quality and recent activity.
    const conversations = [
      ...channelMatches.map(({ channel, match }) => ({
        isDm: channel.channelType === "dm",
        match,
        result: { kind: "channel" as const, channel },
      })),
      ...threadMatches.map(({ isDm, match, thread }) => ({
        isDm,
        match,
        result: { kind: "thread" as const, isDm, thread },
      })),
    ].sort((a, b) => compareGroupMatches(a.match, b.match));
    return [
      ...conversations.filter(({ isDm }) => !isDm).map(({ result }) => result),
      ...conversations.filter(({ isDm }) => isDm).map(({ result }) => result),
      ...userResults.map((user) => ({
        kind: "user" as const,
        user,
      })),
      ...messageResults.map((hit) => ({
        kind: "message" as const,
        hit,
        thread: hitThreads.threadByEventId.get(hit.eventId) ?? null,
      })),
    ];
  }, [
    channelMatches,
    hitThreads.threadByEventId,
    messageResults,
    threadMatches,
    userResults,
  ]);

  const resultProfilesQuery = useUsersBatchQuery(
    messageResults.map((hit) => hit.pubkey),
    {
      enabled: enabled && messageResults.length > 0,
    },
  );

  React.useEffect(() => {
    const trimmed = query.trim();
    if (trimmed.length < minimumQueryLength) {
      setDebouncedQuery("");
      return;
    }

    const timeout = window.setTimeout(() => {
      setDebouncedQuery(trimmed);
    }, 300);

    return () => {
      window.clearTimeout(timeout);
    };
  }, [minimumQueryLength, query]);

  React.useEffect(() => {
    if (!enabled) {
      setQuery("");
      setDebouncedQuery("");
      setSelectedIndex(0);
    }
  }, [enabled]);

  React.useEffect(() => {
    setSelectedIndex((current) => {
      if (results.length === 0) {
        return 0;
      }

      return Math.min(current, results.length - 1);
    });
  }, [results]);

  return {
    channelLookup,
    channelResults,
    debouncedQuery,
    isThreadContextLoading: hitThreads.isLoading,
    isWaitingOnFromResolution: waitingOnFromResolution,
    messageResults,
    query,
    resultProfiles: resultProfilesQuery.data?.profiles,
    results,
    searchQuery,
    selectedIndex,
    selectedResult: results[selectedIndex],
    setQuery,
    setSelectedIndex,
    userResults,
    fuzzyUserCandidatesQuery,
    userSearchQuery,
  };
}
