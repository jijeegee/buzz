import { useQueryClient } from "@tanstack/react-query";
import * as React from "react";
import { toast } from "sonner";

import { mergeMessages } from "@/features/messages/lib/messageMerge";
import { threadRepliesKey } from "@/features/messages/lib/messageQueryKeys";
import {
  THREAD_CLOSED_CONTENT,
  THREAD_CLOSED_TAG,
  THREAD_REOPENED_CONTENT,
  THREAD_REOPENED_TAG,
} from "@/features/messages/lib/taskThread";
import {
  sendThreadMarker,
  type ThreadMarker,
  type ThreadMarkerScope,
} from "@/features/messages/lib/threadMarkerSend";
import { buildReplyTags } from "@/features/messages/lib/threading";
import { normalizeRelayUrl } from "@/features/communities/communityStorage";
import { useCommunities } from "@/features/communities/useCommunities";
import { useIdentityQuery } from "@/shared/api/hooks";
import { sendChannelMessage } from "@/shared/api/tauriMessages";
import type { RelayEvent } from "@/shared/api/types";
import { KIND_STREAM_MESSAGE } from "@/shared/constants/kinds";

/**
 * The thread Close toggle: a marker reply in the thread, the same one
 * `kuzz threads close` / `reopen` write, so apps that predate the button
 * read it too. The sent reply goes into the thread cache right away (the
 * relay echo merges by id). Closing offers Undo, which sends a reopen.
 */
export function useThreadCloseToggle(): {
  pending: boolean;
  setThreadClosed: (input: {
    channelId: string;
    rootId: string;
    closed: boolean;
    /** After each marker reply is sent, including an Undo. */
    onSent?: () => void;
  }) => void;
} {
  const queryClient = useQueryClient();
  const identityQuery = useIdentityQuery();
  const pubkey = identityQuery.data?.pubkey;
  const [pending, setPending] = React.useState(false);

  const { activeCommunity } = useCommunities();
  const relayUrl = activeCommunity?.relayUrl
    ? normalizeRelayUrl(activeCommunity.relayUrl)
    : null;

  const send = React.useCallback(
    async (
      channelId: string,
      rootId: string,
      marker: ThreadMarker,
      scope: ThreadMarkerScope,
    ) => {
      const signer = scope.signerPubkey;
      const result = await sendThreadMarker(sendChannelMessage, {
        channelId,
        marker,
        rootId,
        scope,
      });
      if (!signer) return;
      const content =
        marker === "closed" ? THREAD_CLOSED_CONTENT : THREAD_REOPENED_CONTENT;
      const event: RelayEvent = {
        id: result.eventId,
        pubkey: signer,
        created_at: result.createdAt,
        kind: KIND_STREAM_MESSAGE,
        tags: [
          ...buildReplyTags(channelId, signer, rootId, rootId),
          [
            marker === "closed" ? THREAD_CLOSED_TAG : THREAD_REOPENED_TAG,
            rootId,
          ],
        ],
        content,
        sig: "",
      };
      queryClient.setQueryData<RelayEvent[]>(
        threadRepliesKey(channelId, rootId),
        (current = []) => mergeMessages(current, event),
      );
    },
    [queryClient],
  );

  const setThreadClosed = React.useCallback(
    ({
      channelId,
      rootId,
      closed,
      onSent,
    }: {
      channelId: string;
      rootId: string;
      closed: boolean;
      onSent?: () => void;
    }) => {
      // Bound to the account active now; Undo reuses the same binding, so a
      // switch in between makes it fail instead of signing as the new one.
      const scope: ThreadMarkerScope = { relayUrl, signerPubkey: pubkey };
      setPending(true);
      send(channelId, rootId, closed ? "closed" : "reopened", scope)
        .then(() => {
          onSent?.();
          if (!closed) return;
          toast("Thread closed", {
            action: {
              label: "Undo",
              onClick: () => {
                send(channelId, rootId, "reopened", scope)
                  .then(() => onSent?.())
                  .catch(reportFailure);
              },
            },
          });
        })
        .catch(reportFailure)
        .finally(() => setPending(false));
    },
    [pubkey, relayUrl, send],
  );

  return { pending, setThreadClosed };
}

function reportFailure(error: unknown) {
  toast.error(
    error instanceof Error ? error.message : "Could not update the thread",
  );
}
