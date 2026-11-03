import * as React from "react";
import { useQueryClient } from "@tanstack/react-query";

import { channelsQueryKey } from "@/features/channels/hooks";
import { noteChannelMembershipChange } from "@/shared/api/channelMembershipWrites";
import { getChannelIdFromTags } from "@/features/messages/lib/threading";
import { historyThenLive } from "@/shared/api/historyThenLive";
import { relayClient } from "@/shared/api/relayClient";
import { meLive } from "@/shared/api/relayMeLive";
import type { RelayEvent } from "@/shared/api/types";
import {
  KIND_MEMBER_ADDED_NOTIFICATION,
  KIND_MEMBER_REMOVED_NOTIFICATION,
} from "@/shared/constants/kinds";

const MEMBERSHIP_NOTIFICATION_RETRY_BASE_MS = 1_000;
const MEMBERSHIP_NOTIFICATION_RETRY_MAX_MS = 30_000;
/** The first attach reads notices from just before it went live. */
const MEMBERSHIP_NOTIFICATION_LOOKBACK_SECONDS = 30;
const MEMBERSHIP_NOTIFICATION_FETCH_LIMIT = 50;

/**
 * Membership notices addressed to me. The listener rides the addressed
 * account bundle (`limit: 0`); once it is live, one fetch reads the notices
 * of the last 30 seconds, through a shared seen gate so each notice is
 * handled once. A catch-up that may have missed notices (onGap) refetches
 * the channel list instead.
 */
export function useMembershipNotifications(currentPubkey?: string) {
  const queryClient = useQueryClient();
  const normalizedCurrentPubkey = currentPubkey?.trim().toLowerCase() ?? "";

  const handleMembershipNotification = React.useEffectEvent(
    (event: RelayEvent) => {
      const channelId = getChannelIdFromTags(event.tags);

      void queryClient.invalidateQueries({ queryKey: channelsQueryKey });
      if (!channelId) {
        return;
      }

      void queryClient.invalidateQueries({
        queryKey: ["channels", channelId, "detail"],
      });
      noteChannelMembershipChange(channelId);
    },
  );

  React.useEffect(() => {
    if (normalizedCurrentPubkey.length === 0) {
      return;
    }

    let isCancelled = false;
    let retryTimeout: number | undefined;
    let retryAttempt = 0;
    let dispose: (() => Promise<void>) | undefined;

    const kinds = [
      KIND_MEMBER_ADDED_NOTIFICATION,
      KIND_MEMBER_REMOVED_NOTIFICATION,
    ];
    const subscribe = async (): Promise<boolean> => {
      try {
        const live = await historyThenLive({
          listen: (apply) =>
            meLive.subscribe(
              { kinds, "#p": [normalizedCurrentPubkey], limit: 0 },
              apply,
              undefined,
              {
                onGap: () => {
                  if (isCancelled) return;
                  void queryClient.invalidateQueries({
                    queryKey: channelsQueryKey,
                  });
                },
              },
            ),
          fetch: () =>
            relayClient.fetchEvents({
              kinds,
              "#p": [normalizedCurrentPubkey],
              since:
                Math.floor(Date.now() / 1_000) -
                MEMBERSHIP_NOTIFICATION_LOOKBACK_SECONDS,
              limit: MEMBERSHIP_NOTIFICATION_FETCH_LIMIT,
            }),
          apply: (event) => {
            if (!isCancelled) {
              handleMembershipNotification(event);
            }
          },
          onFetchError: (error) =>
            console.error("Failed to fetch membership notifications", error),
        });
        if (isCancelled) {
          void live.stop().catch(() => {});
          return true;
        }
        dispose = live.stop;
        return true;
      } catch (error) {
        console.error("Failed to subscribe to membership notifications", error);
        return false;
      }
    };

    const run = async () => {
      const ok = await subscribe();
      if (isCancelled || ok) {
        return;
      }

      const delayMs = Math.min(
        MEMBERSHIP_NOTIFICATION_RETRY_BASE_MS * 2 ** retryAttempt,
        MEMBERSHIP_NOTIFICATION_RETRY_MAX_MS,
      );
      retryAttempt += 1;
      retryTimeout = window.setTimeout(() => {
        retryTimeout = undefined;
        void run();
      }, delayMs);
    };

    void run();

    return () => {
      isCancelled = true;
      if (retryTimeout !== undefined) {
        window.clearTimeout(retryTimeout);
      }
      if (dispose) {
        void dispose().catch(() => {});
      }
    };
  }, [normalizedCurrentPubkey, queryClient]);
}
