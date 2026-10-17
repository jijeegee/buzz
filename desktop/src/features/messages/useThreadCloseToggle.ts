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
import { buildReplyTags } from "@/features/messages/lib/threading";
import { useIdentityQuery } from "@/shared/api/hooks";
import { sendChannelMessage } from "@/shared/api/tauriMessages";
import type { RelayEvent } from "@/shared/api/types";
import { KIND_STREAM_MESSAGE } from "@/shared/constants/kinds";

export type ThreadMarker = "closed" | "reopened";

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
  }) => void;
} {
  const queryClient = useQueryClient();
  const identityQuery = useIdentityQuery();
  const pubkey = identityQuery.data?.pubkey;
  const [pending, setPending] = React.useState(false);

  const send = React.useCallback(
    async (channelId: string, rootId: string, marker: ThreadMarker) => {
      if (!pubkey) throw new Error("No identity available.");
      const content =
        marker === "closed" ? THREAD_CLOSED_CONTENT : THREAD_REOPENED_CONTENT;
      const result = await sendChannelMessage(
        channelId,
        content,
        rootId,
        undefined,
        undefined,
        undefined,
        undefined,
        undefined,
        undefined,
        undefined,
        undefined,
        undefined,
        rootId,
        undefined,
        marker,
      );
      const event: RelayEvent = {
        id: result.eventId,
        pubkey,
        created_at: result.createdAt,
        kind: KIND_STREAM_MESSAGE,
        tags: [
          ...buildReplyTags(channelId, pubkey, rootId, rootId),
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
    [pubkey, queryClient],
  );

  const setThreadClosed = React.useCallback(
    ({
      channelId,
      rootId,
      closed,
    }: {
      channelId: string;
      rootId: string;
      closed: boolean;
    }) => {
      setPending(true);
      send(channelId, rootId, closed ? "closed" : "reopened")
        .then(() => {
          if (!closed) return;
          toast("Thread closed", {
            action: {
              label: "Undo",
              onClick: () => {
                send(channelId, rootId, "reopened").catch(reportFailure);
              },
            },
          });
        })
        .catch(reportFailure)
        .finally(() => setPending(false));
    },
    [send],
  );

  return { pending, setThreadClosed };
}

function reportFailure(error: unknown) {
  toast.error(
    error instanceof Error ? error.message : "Could not update the thread",
  );
}
