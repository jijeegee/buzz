import { useQueryClient } from "@tanstack/react-query";
import { Quote } from "lucide-react";

import { useAppNavigation } from "@/app/navigation/useAppNavigation";
import { getQuoteReference } from "@/features/messages/lib/messageQuote";
import {
  findLoadedQuotedEvent,
  type QuotedPreview,
  quotedPreviewFromEvent,
} from "@/features/messages/lib/messageQuotePreview";
import { channelMessagesKey } from "@/features/messages/lib/messageQueryKeys";
import type { UserProfileLookup } from "@/features/profile/lib/identity";
import type { RelayEvent } from "@/shared/api/types";
import { cn } from "@/shared/lib/cn";
import { useMessageLinkMetadata } from "@/shared/ui/markdown/useMessageLinkMetadata";

const QUOTE_HEADER_CLASS =
  "mb-1 flex w-fit max-w-full min-w-0 items-center gap-1.5 rounded-md border-l-2 border-border bg-muted/40 py-0.5 pl-2 pr-2.5 text-left text-sm leading-5 text-muted-foreground";

/**
 * Compact header for a message that carries a NIP-18 `q` tag. Resolves the
 * quoted message from already-loaded timeline/thread caches first and falls
 * back to an event lookup by id; clicking jumps to the original message.
 */
export function MessageQuoteHeader({
  channelId,
  profiles,
  tags,
}: {
  channelId?: string | null;
  profiles?: UserProfileLookup;
  tags?: string[][];
}) {
  const queryClient = useQueryClient();
  const { goChannel } = useAppNavigation();
  const reference = getQuoteReference(tags);
  const loadedEvent =
    reference && channelId
      ? findLoadedQuotedEvent(reference.eventId, [
          queryClient.getQueryData<RelayEvent[]>(channelMessagesKey(channelId)),
          ...queryClient
            .getQueriesData<RelayEvent[]>({
              queryKey: ["thread-replies", channelId],
            })
            .map(([, events]) => events),
        ])
      : null;
  const metadata = useMessageLinkMetadata(
    {
      channelId: channelId ?? "",
      messageId: reference?.eventId ?? "",
      threadRootId: null,
    },
    Boolean(reference && channelId && !loadedEvent),
  );

  if (!reference || !channelId) return null;

  let preview: QuotedPreview;
  if (loadedEvent) {
    preview = quotedPreviewFromEvent(reference, loadedEvent, profiles);
  } else if (metadata.state.kind === "ready") {
    preview = {
      kind: "ready",
      author: metadata.state.author,
      excerpt: metadata.state.snippet,
      threadRootId: metadata.state.threadRootId,
    };
  } else if (
    metadata.state.kind === "deleted" ||
    metadata.state.kind === "unavailable"
  ) {
    preview = { kind: "unavailable" };
  } else {
    preview = { kind: "loading" };
  }

  if (preview.kind !== "ready") {
    return (
      <div
        className={cn(QUOTE_HEADER_CLASS, "italic")}
        data-testid="message-quote-header"
      >
        <Quote aria-hidden className="h-3.5 w-3.5 shrink-0" />
        <span className="truncate">
          {preview.kind === "loading"
            ? "Loading quoted message…"
            : "Original message unavailable"}
        </span>
      </div>
    );
  }

  const quotedEventId = reference.eventId;
  const threadRootId =
    preview.threadRootId && preview.threadRootId !== quotedEventId
      ? preview.threadRootId
      : null;
  return (
    <button
      aria-label={`Jump to quoted message from ${preview.author}`}
      className={cn(
        QUOTE_HEADER_CLASS,
        "transition-colors hover:bg-muted/70 hover:text-foreground focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring",
      )}
      data-testid="message-quote-header"
      onClick={() => {
        void goChannel(channelId, { messageId: quotedEventId, threadRootId });
      }}
      type="button"
    >
      <Quote aria-hidden className="h-3.5 w-3.5 shrink-0" />
      <span className="shrink-0 font-medium text-foreground">
        {preview.author}
      </span>
      <span className="min-w-0 truncate">{preview.excerpt}</span>
    </button>
  );
}
