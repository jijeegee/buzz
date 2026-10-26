import * as React from "react";
import { type QueryClient, useQueryClient } from "@tanstack/react-query";

import type { SearchHighlightNavigation } from "@/app/navigation/searchHighlightNavigation";
import { getCachedSearchHitEvent } from "@/app/navigation/searchHitEventCache";
import { useAppNavigation } from "@/app/navigation/useAppNavigation";
import { channelFromAgentChatRow } from "@/features/agents/ownerView/agentChatList";
import type { OwnerViewChat } from "@/features/agents/ownerView/OwnerViewLayout";
import { OwnerViewBar } from "@/features/agents/ownerView/OwnerViewBar";
import { useChannelsQuery } from "@/features/channels/hooks";
import {
  isChannelReferenceOpenable,
  useChannelReferenceStatus,
} from "@/features/channels/openChannelDirectory";
import { ChannelScreen } from "@/features/channels/ui/ChannelScreen";
import { HuddleStartingView } from "@/features/huddle/components/HuddleStartingView";
import { huddleWindowChannelId } from "@/features/huddle/lib/huddleWindow";
import { getRememberedRouteEvent } from "@/features/messages/lib/routeEventCache";
import {
  getChannelIdFromTags,
  getThreadReference,
  isBroadcastReply,
} from "@/features/messages/lib/threading";
import { useProfileQuery } from "@/features/profile/hooks";
import {
  useProjectHomeForChannelQuery,
  useProjectsQuery,
} from "@/features/projects/hooks";
import { findProjectHomeByChannelId } from "@/features/projects/lib/projectHomeChannel";
import {
  isProjectCollectionAuthoritative,
  isProjectRelayValidated,
  shouldUseScopedProjectHomeLookup,
} from "@/features/projects/projectSnapshot";
import { ProjectChannelHome } from "@/features/projects/ui/ProjectChannelHome";
import { useIdentityQuery } from "@/shared/api/hooks";
import { getEventById } from "@/shared/api/tauri";
import type { Channel, RelayEvent } from "@/shared/api/types";
import { useT } from "@/shared/i18n";
import { ViewLoadingFallback } from "@/shared/ui/ViewLoadingFallback";

type ChannelRouteScreenProps = {
  autoSendDraftKey: string | null;
  channelId: string;
  /** Owner view: read this chat of my agent, without being in it. */
  ownerView?: OwnerViewChat | null;
  searchHighlight: SearchHighlightNavigation | null | undefined;
  selectedPostId: string | null;
  targetMessageId: string | null;
  targetReplyId: string | null;
  targetThreadRootId: string | null;
};

const MAX_ROUTE_ANCESTOR_HOPS = 50;
/** A route fetch that failed (not "not found") is tried again this often. */
const MAX_ROUTE_FETCH_RETRIES = 5;
const routeRetryDelayMs = (attempt: number) =>
  Math.min(1_000 * 2 ** attempt, 15_000);

const NO_EVENTS: RelayEvent[] = [];

/** Route target events of one room, and the target key last settled there. */
type RouteRecord = {
  channelId: string;
  events: RelayEvent[];
  resolvedKey: string | null;
};

/** An event of `channelId` (or one carrying no room, scoped by the relay). */
function isRoomEvent(event: RelayEvent, channelId: string): boolean {
  const room = getChannelIdFromTags(event.tags);
  return room === null || room === channelId;
}

/** `record` for `channelId` with `events` of that room added (by id). */
function withRouteEvents(
  record: RouteRecord,
  channelId: string,
  events: readonly RelayEvent[],
): RouteRecord {
  const base: RouteRecord =
    record.channelId === channelId
      ? record
      : { channelId, events: [], resolvedKey: null };
  const eventsById = new Map(base.events.map((event) => [event.id, event]));
  for (const event of events) {
    if (isRoomEvent(event, channelId)) eventsById.set(event.id, event);
  }
  return { ...base, events: Array.from(eventsById.values()) };
}

/** Set when a fetch failed for a reason other than the event not existing. */
type RouteFetchFailure = { failed: boolean };

async function fetchRouteEvent(
  queryClient: QueryClient,
  eventId: string,
  failure: RouteFetchFailure,
): Promise<RelayEvent | null> {
  // Preloaded thread replies and roots open without a relay round trip; the
  // room and thread queries still revalidate them.
  const remembered = getRememberedRouteEvent(queryClient, eventId);
  if (remembered) return remembered;
  try {
    return await getEventById(eventId);
  } catch (error) {
    console.error("Failed to load route event", eventId, error);
    // Only "not found" settles the route as missing; anything else is
    // retried, so a thread is not closed for a passing network error.
    if (!String(error).includes("event not found")) failure.failed = true;
    return null;
  }
}

function getReplyParentId(event: RelayEvent): string | null {
  if (isBroadcastReply(event.tags)) {
    return null;
  }

  return getThreadReference(event.tags).parentId;
}

async function fetchRouteTargetEvents(
  queryClient: QueryClient,
  eventIds: string[],
  targetMessageId: string | null,
  targetThreadRootId: string | null,
  failure: RouteFetchFailure,
): Promise<RelayEvent[]> {
  const eventsById = new Map<string, RelayEvent>();
  const addEvent = (event: RelayEvent | null) => {
    if (event) {
      eventsById.set(event.id, event);
    }
  };

  const uniqueEventIds = [...new Set(eventIds)];
  const initialEvents = await Promise.all(
    uniqueEventIds.map((eventId) =>
      fetchRouteEvent(queryClient, eventId, failure),
    ),
  );
  for (const event of initialEvents) {
    addEvent(event);
  }

  const targetEvent = targetMessageId
    ? (eventsById.get(targetMessageId) ?? null)
    : null;
  if (!targetEvent) {
    return [...eventsById.values()];
  }

  const targetThreadRef = getThreadReference(targetEvent.tags);
  const threadRootId = targetThreadRootId ?? targetThreadRef.rootId ?? null;
  if (threadRootId && !eventsById.has(threadRootId)) {
    addEvent(await fetchRouteEvent(queryClient, threadRootId, failure));
  }

  let parentId = getReplyParentId(targetEvent);
  let guard = 0;
  while (
    parentId &&
    parentId !== threadRootId &&
    guard < MAX_ROUTE_ANCESTOR_HOPS
  ) {
    const parentEvent =
      eventsById.get(parentId) ??
      (await fetchRouteEvent(queryClient, parentId, failure));
    if (!parentEvent) {
      break;
    }

    eventsById.set(parentEvent.id, parentEvent);
    parentId = getReplyParentId(parentEvent);
    guard += 1;
  }

  return [...eventsById.values()];
}

export function ChannelRouteScreen({
  autoSendDraftKey,
  channelId,
  ownerView = null,
  searchHighlight,
  selectedPostId,
  targetMessageId,
  targetReplyId,
  targetThreadRootId,
}: ChannelRouteScreenProps) {
  const t = useT();
  const isHuddleTranscript = huddleWindowChannelId() !== null;
  const queryClient = useQueryClient();
  const { closeForumPost, goForumPost } = useAppNavigation();
  // Subscribe to this room's row only: every live event rewrites the list's
  // recency, and the open room must not re-render for other rooms.
  const selectRouteChannel = React.useCallback(
    (channels: Channel[]) =>
      channels.find((channel) => channel.id === channelId) ?? null,
    [channelId],
  );
  const channelsQuery = useChannelsQuery({ select: selectRouteChannel });
  const projectsQuery = useProjectsQuery();
  const identityQuery = useIdentityQuery();
  const profileQuery = useProfileQuery();
  const ownerViewRow = ownerView?.row ?? null;
  const ownerViewAgent = ownerView?.agentPubkey ?? null;
  // Rebuilt only when who is in the chat changes, not on every list refresh.
  const ownerViewKey = ownerViewRow
    ? [
        ownerViewRow.channel_id,
        ownerViewRow.channel_type,
        ownerViewRow.channel_name,
        ownerViewRow.group_chat === true,
        ownerViewRow.member_count ?? 0,
        ownerViewRow.dm_members.join(","),
      ].join("|")
    : null;
  // biome-ignore lint/correctness/useExhaustiveDependencies: keyed by the chat's people, see ownerViewKey
  const ownerViewChannel = React.useMemo(
    () =>
      ownerViewRow && ownerViewAgent
        ? channelFromAgentChatRow(ownerViewRow, ownerViewAgent)
        : null,
    [ownerViewAgent, ownerViewKey],
  );
  const memberChannel = ownerViewChannel ? null : (channelsQuery.data ?? null);
  // A non-member public group (a Find groups preview or a deep link) is not in
  // the member-only poll list. Resolve just that one group by id — never the
  // all-open directory — and only when it is openable (open, or a member).
  const reference = useChannelReferenceStatus(
    !memberChannel &&
      !ownerViewChannel &&
      channelsQuery.isSuccess &&
      !isHuddleTranscript
      ? channelId
      : null,
  );
  const referencedChannel = reference.channel;
  const activeChannel =
    ownerViewChannel ??
    memberChannel ??
    (isChannelReferenceOpenable(referencedChannel) ? referencedChannel : null);
  const enumeratedProjectHome = findProjectHomeByChannelId(
    channelId,
    projectsQuery.data ?? [],
  );
  const projectCollectionIsAuthoritative =
    isProjectCollectionAuthoritative(queryClient);
  const projectHomeLookupQuery = useProjectHomeForChannelQuery(
    channelId,
    shouldUseScopedProjectHomeLookup({
      collectionIsAuthoritative: projectCollectionIsAuthoritative,
      hasEnumeratedProjectHome: Boolean(enumeratedProjectHome),
      isHuddleTranscript: isHuddleTranscript || ownerViewChannel !== null,
    }),
  );
  const projectHome = ownerViewChannel
    ? null
    : (enumeratedProjectHome ?? projectHomeLookupQuery.data ?? null);
  // Events fetched for this room's route target, and the target the last
  // fetch settled for, held together with their room: this screen stays
  // mounted across rooms, so another room's record is never shown here and
  // never counts as settled for this one. Until it settles, a thread opened by
  // this route waits for its root instead of closing as missing.
  const [routeRecord, setRouteRecord] = React.useState<RouteRecord>(() => {
    const cachedTarget = getCachedSearchHitEvent(targetMessageId);
    return {
      channelId,
      events: cachedTarget ? [cachedTarget] : [],
      resolvedKey: null,
    };
  });
  const targetMessageEvents =
    routeRecord.channelId === channelId ? routeRecord.events : NO_EVENTS;
  const resolvedRouteTargetKey =
    routeRecord.channelId === channelId ? routeRecord.resolvedKey : null;
  const routeTargetKey =
    !selectedPostId && (targetMessageId || targetThreadRootId)
      ? `${channelId}|${targetMessageId ?? ""}|${targetThreadRootId ?? ""}`
      : null;
  // A failed fetch is tried again (see `fetchRouteEvent`); the attempt is
  // tied to the target it was for.
  const [routeRetry, setRouteRetry] = React.useState<{
    key: string | null;
    attempt: number;
  }>({ key: null, attempt: 0 });
  const routeAttempt =
    routeRetry.key === routeTargetKey ? routeRetry.attempt : 0;
  const isRouteTargetResolving =
    routeTargetKey !== null && resolvedRouteTargetKey !== routeTargetKey;
  // Preloaded roots and replies are at hand on the first render, so a thread
  // opened from the inbox shows its root without waiting a render for them.
  const displayedTargetMessageEvents = React.useMemo(() => {
    const remembered = [targetThreadRootId, targetMessageId].flatMap((id) => {
      const event = id ? getRememberedRouteEvent(queryClient, id) : null;
      return event && isRoomEvent(event, channelId) ? [event] : [];
    });
    if (remembered.length === 0) return targetMessageEvents;
    const eventsById = new Map<string, RelayEvent>();
    for (const event of [...remembered, ...targetMessageEvents]) {
      eventsById.set(event.id, event);
    }
    return Array.from(eventsById.values());
  }, [
    channelId,
    queryClient,
    targetMessageEvents,
    targetMessageId,
    targetThreadRootId,
  ]);
  const [activeSearchHighlight, setActiveSearchHighlight] =
    React.useState<SearchHighlightNavigation | null>(searchHighlight ?? null);
  const appliedSearchActivationIdRef = React.useRef<string | null>(
    searchHighlight?.activationId ?? null,
  );

  // Router state is transient and can be cleared by the target URL cleanup.
  // Retain the applied activation locally until an ordinary route transition
  // explicitly arrives without search state.
  React.useEffect(() => {
    if (searchHighlight === null) {
      appliedSearchActivationIdRef.current = null;
      setActiveSearchHighlight(null);
      return;
    }
    if (!searchHighlight) {
      const ordinaryTargetIds = [
        selectedPostId,
        targetMessageId,
        targetReplyId,
        targetThreadRootId,
      ].filter((targetId): targetId is string => targetId !== null);
      if (
        ordinaryTargetIds.length > 0 &&
        activeSearchHighlight &&
        !ordinaryTargetIds.includes(activeSearchHighlight.messageId)
      ) {
        appliedSearchActivationIdRef.current = null;
        setActiveSearchHighlight(null);
      }
      return;
    }
    if (appliedSearchActivationIdRef.current === searchHighlight.activationId) {
      return;
    }

    appliedSearchActivationIdRef.current = searchHighlight.activationId;
    setActiveSearchHighlight(searchHighlight);
  }, [
    activeSearchHighlight,
    searchHighlight,
    selectedPostId,
    targetMessageId,
    targetReplyId,
    targetThreadRootId,
  ]);

  // Reset the search highlight when the channel changes. Spliced target
  // events need no reset: `routeRecord` is tied to its room (see above).
  const previousResetKeyRef = React.useRef<string>(channelId);
  React.useEffect(() => {
    if (previousResetKeyRef.current === channelId) return;
    previousResetKeyRef.current = channelId;
    appliedSearchActivationIdRef.current = null;
    setActiveSearchHighlight(null);
  }, [channelId]);

  React.useEffect(() => {
    let isCancelled = false;

    // Don't wipe already-spliced target events just because the route target
    // cleared (e.g. `onTargetReached` clears the `messageId` URL param once the
    // row is centered). In a channel whose feed doesn't already contain the
    // deep-linked message, the spliced event is the only copy — dropping it on
    // param-clear blanks the timeline. Resetting on channel / forum-post change
    // is handled by the effect below; here we only fetch when there's a target.
    if ((!targetMessageId && !targetThreadRootId) || selectedPostId) {
      return () => {
        isCancelled = true;
      };
    }

    const cachedTarget = getCachedSearchHitEvent(targetMessageId);
    if (cachedTarget) {
      setRouteRecord((record) =>
        withRouteEvents(record, channelId, [cachedTarget]),
      );
    }

    const eventIds = [
      targetMessageId,
      targetThreadRootId && targetThreadRootId !== targetMessageId
        ? targetThreadRootId
        : null,
    ].filter((eventId): eventId is string => eventId !== null);

    const key = `${channelId}|${targetMessageId ?? ""}|${targetThreadRootId ?? ""}`;
    const failure = { failed: false };
    let retryTimer: number | undefined;
    void fetchRouteTargetEvents(
      queryClient,
      eventIds,
      targetMessageId,
      targetThreadRootId,
      failure,
    ).then((events) => {
      if (isCancelled) return;
      // A failed fetch keeps the target unresolved and tries again; only a
      // complete answer (or the last attempt) settles it.
      const retry = failure.failed && routeAttempt < MAX_ROUTE_FETCH_RETRIES;
      setRouteRecord((record) => {
        const next = withRouteEvents(record, channelId, events);
        return retry ? next : { ...next, resolvedKey: key };
      });
      if (retry) {
        retryTimer = window.setTimeout(() => {
          setRouteRetry({ key, attempt: routeAttempt + 1 });
        }, routeRetryDelayMs(routeAttempt));
      }
    });

    return () => {
      isCancelled = true;
      window.clearTimeout(retryTimer);
    };
  }, [
    channelId,
    queryClient,
    routeAttempt,
    selectedPostId,
    targetMessageId,
    targetThreadRootId,
  ]);

  const screenOwnerView = React.useMemo(
    () =>
      ownerView && ownerViewChannel
        ? {
            agentName: ownerView.agentName,
            agentPubkey: ownerView.agentPubkey,
            footer: (chatLabel: string) => (
              <OwnerViewBar
                agentName={ownerView.agentName}
                agentPubkey={ownerView.agentPubkey}
                channelId={ownerViewChannel.id}
                chatLabel={chatLabel}
                onRemoved={ownerView.onRemoved}
              />
            ),
            subtitle:
              ownerViewChannel.channelType === "dm"
                ? t("ownerView.header.dm")
                : t("ownerView.header.groupChat", {
                    count: ownerViewChannel.memberCount,
                  }),
          }
        : undefined,
    [ownerView, ownerViewChannel, t],
  );
  const handleCloseForumPost = React.useCallback(() => {
    void closeForumPost(channelId);
  }, [channelId, closeForumPost]);
  const handleSelectForumPost = React.useCallback(
    (postId: string) => {
      void goForumPost(channelId, postId);
    },
    [channelId, goForumPost],
  );

  if (!activeChannel && (channelsQuery.isPending || reference.isResolving)) {
    if (isHuddleTranscript) {
      return <HuddleStartingView />;
    }
    return (
      <ViewLoadingFallback
        includeHeader
        kind={selectedPostId ? "forum" : "channel"}
      />
    );
  }

  if (projectHome && !isHuddleTranscript) {
    return (
      <ProjectChannelHome
        allowRepositoryHealing={isProjectRelayValidated(projectHome)}
        autoSendDraftKey={autoSendDraftKey}
        project={projectHome}
        projects={projectsQuery.data ?? [projectHome]}
        targetMessageEvents={displayedTargetMessageEvents}
        targetMessageId={targetMessageId}
      />
    );
  }

  return (
    <ChannelScreen
      activeChannel={activeChannel}
      ownerView={screenOwnerView}
      autoSendDraftKey={autoSendDraftKey}
      currentIdentity={identityQuery.data}
      currentProfile={profileQuery.data}
      onCloseForumPost={handleCloseForumPost}
      onSelectForumPost={handleSelectForumPost}
      selectedForumPostId={selectedPostId}
      targetForumReplyId={targetReplyId}
      isRouteTargetResolving={isRouteTargetResolving}
      targetMessageEvents={displayedTargetMessageEvents}
      targetMessageId={targetMessageId}
      targetSearchMessageId={activeSearchHighlight?.messageId}
      targetSearchQuery={activeSearchHighlight?.query}
    />
  );
}
