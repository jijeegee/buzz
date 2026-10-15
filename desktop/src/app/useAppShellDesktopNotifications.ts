import * as React from "react";
import { toast } from "sonner";

import {
  activateDesktopNotificationTarget,
  createDesktopNotificationActivationQueue,
  shouldBounceForChannelNotification,
} from "@/app/AppShell.helpers";
import {
  type OpenForAccountDeps,
  openNotificationForAccount,
  resumeNotificationForAccount,
} from "@/features/accounts/inactiveAccountWatch/openForAccount";
import { relayOrigin } from "@/features/accounts/inactiveAccountWatch/targets";
import { useInactiveAccountWatch } from "@/features/accounts/inactiveAccountWatch/useInactiveAccountWatch";
import {
  beginAccountAction,
  SWITCH_INCOMPLETE_NOTICE,
} from "@/features/accounts/lib/accountSwitch";
import {
  isAllowedServer,
  useBuiltInServer,
} from "@/features/communities/builtInServer";
import { useCommunities } from "@/features/communities/useCommunities";
import { useCommunityJoinAlerts } from "@/features/community-members/useCommunityJoinAlerts";
import { hasMentionForEvent } from "@/features/notifications/lib/shouldNotify";
import type { NotificationSettings } from "@/features/notifications/hooks";
import {
  listenForDesktopNotificationActions,
  requestDockBounce,
  revealDesktopAppWindow,
  sendDesktopNotification,
} from "@/features/notifications/lib/desktop";
import { formatMessageNotification } from "@/features/notifications/lib/notificationFormat";
import { buildEventNotificationTarget } from "@/features/notifications/lib/target";
import {
  playNotificationSound,
  resolveSlotSound,
  shouldPlayNotificationSound,
} from "@/features/notifications/lib/sound";
import { useNotificationSenderName } from "@/features/notifications/useNotificationSenderName";
import { listWatchableAccounts, switchAccount } from "@/shared/api/tokenAuth";
import type { Channel, RelayEvent } from "@/shared/api/types";

const UNAVAILABLE_TEXT = {
  signed_out:
    "That account is signed out on this computer. Sign in to it from the profile menu.",
  no_community: "That account's community is no longer on this computer.",
  failed: "Could not switch to that account.",
} as const;

export function useAppShellDesktopNotifications({
  channels,
  enabled,
  goChannel,
  goHome,
  notificationSettings,
  openSearchHit,
  pubkey,
  silentChannelIds,
}: {
  channels: Channel[];
  enabled: boolean;
  goChannel: (
    channelId: string,
    options?: { force?: boolean },
  ) => Promise<unknown>;
  goHome: () => Promise<unknown>;
  notificationSettings: NotificationSettings;
  openSearchHit: (
    hit: import("@/shared/api/types").SearchHit,
    behavior?: { force?: boolean },
  ) => Promise<unknown>;
  pubkey?: string;
  silentChannelIds?: ReadonlySet<string>;
}) {
  // Roster alerts are owner/admin-only and self-gating; mounted here because
  // it shares this hook's "desktop notifications are on" precondition and
  // AppShell sits at the file-size ratchet ceiling.
  useCommunityJoinAlerts({
    enabled: enabled && notificationSettings.desktopEnabled,
  });
  // Other accounts signed in here notify by their own settings.
  useInactiveAccountWatch(enabled);

  const resolveSenderName = useNotificationSenderName();

  const handleChannelNotification = React.useEffectEvent(
    (_channelId: string, event: RelayEvent) => {
      if (!enabled) return;
      if (!shouldBounceForChannelNotification(event.tags)) return;
      if (!notificationSettings.desktopEnabled) return;
      void requestDockBounce();
    },
  );

  const handleDmNotification = React.useEffectEvent(
    (event: RelayEvent, channel: Channel) => {
      if (!enabled) return;
      if (
        !notificationSettings.desktopEnabled ||
        !notificationSettings.slotAlertsEnabled.dm
      ) {
        return;
      }

      const channelName = channel.name?.trim() || "Direct message";
      const { title, body } = formatMessageNotification({
        source: "dm",
        senderName: resolveSenderName(event.pubkey),
        channelName,
        content: event.content,
      });

      void sendDesktopNotification({
        title,
        body,
        target: buildEventNotificationTarget(event, {
          id: channel.id,
          name: channelName,
        }),
      }).then((didSend) => {
        if (!didSend) return;
        if (shouldPlayNotificationSound(channel.id, silentChannelIds)) {
          playNotificationSound(resolveSlotSound(notificationSettings, "dm"));
        }
        void requestDockBounce();
      });
    },
  );

  const handleThreadReplyDesktopNotification = React.useEffectEvent(
    (channelId: string, event: RelayEvent) => {
      if (!enabled) return;
      if (
        !notificationSettings.desktopEnabled ||
        !notificationSettings.slotAlertsEnabled.thread_reply
      ) {
        return;
      }

      // Replies that @-mention the user are owned by the home-feed mention
      // path — skip them here so they don't notify (and sound) twice.
      const normalizedPubkey = pubkey?.trim().toLowerCase() ?? "";
      if (hasMentionForEvent(event, normalizedPubkey)) {
        return;
      }

      const resolvedChannel = channels.find((c) => c.id === channelId);
      const channelName = resolvedChannel?.name?.trim() ?? null;
      const { title, body } = formatMessageNotification({
        source: "thread_reply",
        senderName: resolveSenderName(event.pubkey),
        channelName,
        content: event.content,
      });

      void sendDesktopNotification({
        title,
        body,
        target: buildEventNotificationTarget(event, {
          id: channelId,
          name: channelName,
        }),
      }).then((didSend) => {
        if (!didSend) return;
        if (shouldPlayNotificationSound(channelId, silentChannelIds)) {
          playNotificationSound(
            resolveSlotSound(notificationSettings, "thread_reply"),
          );
        }
        void requestDockBounce();
      });
    },
  );

  const { communities, activeCommunity, switchCommunity } = useCommunities();
  const builtInServer = useBuiltInServer();
  const openOrigin = activeCommunity
    ? relayOrigin(activeCommunity.relayUrl)
    : null;
  const openPubkey = pubkey?.trim().toLowerCase() || null;

  /** How a click reaches an account that is not the open one. */
  const accountRouting = (signal: AbortSignal): OpenForAccountDeps => ({
    open: { origin: openOrigin, pubkey: openPubkey },
    communities,
    isSignedIn: async (account) =>
      (await listWatchableAccounts()).some(
        (candidate) =>
          candidate.origin === account.origin &&
          candidate.pubkey.toLowerCase() === account.pubkey.toLowerCase(),
      ),
    switchCommunity,
    switchAccount: async (target) => {
      const action = beginAccountAction();
      try {
        const incomplete = await switchAccount(target);
        action.reload(
          "Switching accounts…",
          incomplete ? SWITCH_INCOMPLETE_NOTICE : null,
        );
      } catch (error) {
        action.cancel();
        throw error;
      }
    },
    openHere: (target) =>
      activateDesktopNotificationTarget(
        target,
        {
          goChannel,
          goHome,
          openSearchHit,
          revealWindow: revealDesktopAppWindow,
        },
        signal,
      ),
    unavailable: (reason) => {
      void revealDesktopAppWindow().catch(() => undefined);
      toast.error(UNAVAILABLE_TEXT[reason]);
    },
    allowsServer: (origin) => isAllowedServer(origin, builtInServer),
    focusApp: () => {
      void revealDesktopAppWindow().catch(() => undefined);
    },
  });

  const handleDesktopNotificationAction = React.useEffectEvent(
    async (
      target: import("@/features/notifications/lib/desktop").DesktopNotificationTarget,
      signal: AbortSignal,
    ) => {
      await openNotificationForAccount(target, accountRouting(signal));
    },
  );

  // A click that switched accounts restarted the app: take its next step
  // once this start knows the open account.
  const resumeAccountRouting = React.useEffectEvent((signal: AbortSignal) =>
    resumeNotificationForAccount(accountRouting(signal)),
  );
  const resumeReady = enabled && openPubkey !== null;
  // Once per start: a second run (StrictMode) would switch twice.
  const resumed = React.useRef(false);
  React.useEffect(() => {
    if (!resumeReady || resumed.current) return;
    resumed.current = true;
    // Not aborted on cleanup: it runs once, and a StrictMode re-run must not
    // cancel it.
    void resumeAccountRouting(new AbortController().signal).catch((error) => {
      console.error("Failed to continue a notification click", error);
    });
  }, [resumeReady]);

  React.useEffect(() => {
    if (!enabled) return;
    let isCancelled = false;
    let cleanup = () => {};
    const activationQueue = createDesktopNotificationActivationQueue(
      (target, signal) => handleDesktopNotificationAction(target, signal),
      (error) => {
        console.error("Failed to activate desktop notification", error);
      },
    );

    void listenForDesktopNotificationActions((target) => {
      if (isCancelled) {
        return;
      }

      activationQueue.enqueue(target);
    }).then((dispose) => {
      if (isCancelled) {
        dispose();
        return;
      }

      cleanup = dispose;
    });

    return () => {
      isCancelled = true;
      activationQueue.cancel();
      cleanup();
    };
  }, [enabled]);

  return {
    handleChannelNotification,
    handleDmNotification,
    handleThreadReplyDesktopNotification,
  };
}
