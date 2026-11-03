import { type QueryClient, QueryClientContext } from "@tanstack/react-query";
import * as React from "react";

import type { ParsedMessageLink } from "@/features/messages/lib/messageLink";
import { storeScope } from "@/features/messages/lib/paintCoverage";
import { threadRootVerdict } from "@/features/messages/lib/threadRootResolve";
import { summarizeMessageLinkContent } from "@/features/messages/lib/messageLinkMetadata";
import { getThreadReference } from "@/features/messages/lib/threading";
import { getEventById, invokeTauri } from "@/shared/api/tauri";
import type { RelayEvent } from "@/shared/api/types";
import { getUserProfile } from "@/shared/api/tauriProfiles";
import { unknownUserLabel } from "@/shared/lib/pubkey";
import { isDefinitiveEventNotFound } from "@/shared/lib/eventLookupError";

const MESSAGE_METADATA_RETRY_DELAY_MS = 750;
const PREVIEWABLE_MESSAGE_KINDS = new Set([9, 40002, 45001, 45003]);

function waitForMessageMetadataRetry(): Promise<void> {
  return new Promise((resolve) => {
    window.setTimeout(resolve, MESSAGE_METADATA_RETRY_DELAY_MS);
  });
}

async function getMessageLinkEvent(messageId: string) {
  try {
    return await getEventById(messageId);
  } catch (error) {
    if (isDefinitiveEventNotFound(error)) throw error;
    await waitForMessageMetadataRetry();
    return getEventById(messageId);
  }
}

type MessageLinkMetadata = {
  author: string;
  createdAt: number;
  snippet: string;
  /** Thread root of the linked event, or null for a top-level message. */
  threadRootId: string | null;
};
type MessageLinkMetadataState =
  | { kind: "idle" }
  | { kind: "loading" }
  | ({ kind: "ready" } & MessageLinkMetadata)
  | { kind: "deleted" }
  | { kind: "unavailable" };

type CachedMessageLinkMetadata =
  | ({ kind: "ready" } & MessageLinkMetadata)
  | { kind: "deleted" }
  | { kind: "unavailable" };

const metadataCache = new Map<string, Promise<CachedMessageLinkMetadata>>();

export function resetMessageLinkMetadataCache() {
  metadataCache.clear();
}

/**
 * A message not found is deleted only on an explicit deletion of it (a
 * removal, or with its author unknown nothing else); otherwise it is just
 * unavailable, e.g. from before the reader joined a since-join room.
 */
async function notFoundState(
  queryClient: QueryClient | undefined,
  link: Pick<ParsedMessageLink, "channelId" | "messageId">,
): Promise<CachedMessageLinkMetadata> {
  const scope = queryClient ? storeScope(queryClient) : null;
  if (!scope) return { kind: "unavailable" };
  const answer = await invokeTauri<RelayEvent[]>("get_thread_root", {
    channelId: link.channelId,
    rootId: link.messageId,
    account: scope.pubkey,
    relayUrl: scope.relayUrl,
  }).catch(() => null);
  return threadRootVerdict(answer, {
    channelId: link.channelId,
    rootId: link.messageId,
  }).verdict === "deleted"
    ? { kind: "deleted" }
    : { kind: "unavailable" };
}

function fetchMetadata(
  queryClient: QueryClient | undefined,
  link: Pick<ParsedMessageLink, "channelId" | "messageId">,
): Promise<CachedMessageLinkMetadata> {
  const key = `${link.channelId}:${link.messageId}`;
  let request = metadataCache.get(key);
  if (!request) {
    request = getMessageLinkEvent(link.messageId)
      .then(async (event) => {
        const eventChannelId = event.tags.find((tag) => tag[0] === "h")?.[1];
        if (
          eventChannelId !== link.channelId ||
          !PREVIEWABLE_MESSAGE_KINDS.has(event.kind)
        ) {
          return { kind: "unavailable" } as const;
        }
        const profile = await getUserProfile(event.pubkey).catch(() => null);
        return {
          kind: "ready" as const,
          author:
            profile?.displayName?.trim() ||
            profile?.nip05Handle?.trim() ||
            unknownUserLabel(),
          createdAt: event.created_at,
          snippet: summarizeMessageLinkContent(event.content),
          threadRootId: getThreadReference(event.tags).rootId,
        };
      })
      .catch((error) =>
        isDefinitiveEventNotFound(error)
          ? notFoundState(queryClient, link)
          : ({ kind: "unavailable" } as const),
      );
    metadataCache.set(key, request);
    void request.then((result) => {
      if (result.kind === "unavailable" && metadataCache.get(key) === request) {
        metadataCache.delete(key);
      }
    });
  }
  return request;
}

export function useMessageLinkMetadata(
  link: ParsedMessageLink,
  channelReadable: boolean,
): { state: MessageLinkMetadataState } {
  const [resolved, setResolved] = React.useState<{
    key: string;
    state: CachedMessageLinkMetadata;
  } | null>(null);
  // Read without throwing: markdown also renders outside a query provider.
  const queryClient = React.useContext(QueryClientContext);
  const requestId = React.useRef(0);
  const key = `${link.channelId}:${link.messageId}`;
  React.useEffect(() => {
    requestId.current += 1;
    if (!channelReadable) return;

    const currentRequest = requestId.current;
    const currentLink = {
      channelId: link.channelId,
      messageId: link.messageId,
    };
    void fetchMetadata(queryClient, currentLink).then((metadata) => {
      if (requestId.current === currentRequest) {
        setResolved({ key, state: metadata });
      }
    });
    return () => {
      requestId.current += 1;
    };
  }, [channelReadable, key, link.channelId, link.messageId, queryClient]);
  if (!channelReadable) return { state: { kind: "idle" } };
  return {
    state: resolved?.key === key ? resolved.state : { kind: "loading" },
  };
}
