import * as React from "react";
import { listen } from "@tauri-apps/api/event";

import { accountLabel } from "@/features/accounts/lib/accountLabels";
import { defaultReadThreadRelationships } from "@/features/communities/communityUnreadObserver";
import {
  isAllowedServer,
  useBuiltInServer,
} from "@/features/communities/builtInServer";
import { useCommunities } from "@/features/communities/useCommunities";
import { readStoredNotificationSettings } from "@/features/notifications/hooks";
import { sendDesktopNotification } from "@/features/notifications/lib/desktop";
import { formatMessageNotification } from "@/features/notifications/lib/notificationFormat";
import { senderNameFromSummary } from "@/features/notifications/lib/senderName";
import {
  playNotificationSound,
  resolveSlotSound,
} from "@/features/notifications/lib/sound";
import { buildEventNotificationTarget } from "@/features/notifications/lib/target";
import { readCachedUserLabels } from "@/features/profile/lib/userLabelStorage";
import { ReadOnlyRelayClient } from "@/shared/api/readOnlyRelayClient";
import { accountAuthFrame } from "@/shared/api/relayAuthFrame";
import { nip44DecryptAs } from "@/shared/api/tauri";
import { getIdentity } from "@/shared/api/tauriIdentity";
import {
  ACCOUNTS_CHANGED_EVENT,
  listWatchableAccounts,
  type WatchableAccount,
} from "@/shared/api/tokenAuth";
import { installRelayResumeTriggers } from "@/shared/api/useRelayResumeTriggers";

import { relayOrigin, type WatchTarget, watchTargets } from "./targets";
import { InactiveAccountWatcher, type WatcherDeps } from "./watcher";
import { watchedUnreadStore } from "./store";

function senderName(relayUrl: string, pubkey: string): string | null {
  const normalized = pubkey.toLowerCase();
  const summary = readCachedUserLabels(relayUrl, [normalized])?.profiles[
    normalized
  ];
  return summary ? senderNameFromSummary(summary) : null;
}

function watcherDeps(target: WatchTarget): WatcherDeps {
  const pubkey = target.account.pubkey;
  const label = accountLabel(target.account);
  return {
    createClient: (relayUrl, options) =>
      new ReadOnlyRelayClient(relayUrl, options),
    authFrame: accountAuthFrame(pubkey),
    decrypt: (ciphertext) => nip44DecryptAs(pubkey, ciphertext),
    readContext: (account) => ({
      settings: readStoredNotificationSettings(account),
      ...defaultReadThreadRelationships(account),
    }),
    notify: (effect, event, channel) => {
      const slot = effect.notify;
      if (!slot) return;
      const settings = readStoredNotificationSettings(pubkey);
      const { title, body } = formatMessageNotification({
        source: slot,
        senderName: senderName(target.relayUrl, event.pubkey),
        channelName: channel.name ?? null,
        content: event.content,
      });
      void sendDesktopNotification({
        // Which account it is for: it is not the open one.
        title: `${label} · ${title}`,
        body,
        target: {
          ...buildEventNotificationTarget(event, {
            id: channel.id,
            name: channel.name,
          }),
          account: { origin: target.account.origin, pubkey },
        },
      }).then((didSend) => {
        if (didSend) {
          playNotificationSound(resolveSlotSound(settings, slot));
        }
      });
    },
    store: watchedUnreadStore,
    log: (message, error) => console.debug(message, error),
  };
}

/**
 * Background notifications and unread state for every account signed in on
 * this computer except the open one: one long-lived read-only relay session
 * each. Mounted once in the app shell; the list is re-read when accounts
 * change, and sessions that dropped reconnect on resume / online / focus.
 */
export function useInactiveAccountWatch(enabled: boolean): void {
  const { communities, activeCommunity } = useCommunities();
  const [accounts, setAccounts] = React.useState<WatchableAccount[]>([]);
  const [openPubkey, setOpenPubkey] = React.useState<string | null>(null);
  const watchers = React.useRef(new Map<string, InactiveAccountWatcher>());
  const openOrigin = activeCommunity
    ? relayOrigin(activeCommunity.relayUrl)
    : null;

  // The native list leaves out the open account on the open server, so it is
  // re-read when the open server changes as well as when accounts change.
  // biome-ignore lint/correctness/useExhaustiveDependencies: openOrigin only triggers the re-read
  React.useEffect(() => {
    if (!enabled) return;
    let cancelled = false;
    const reload = () => {
      void Promise.all([listWatchableAccounts(), getIdentity()])
        .then(([next, identity]) => {
          if (cancelled) return;
          setAccounts(next);
          setOpenPubkey(identity.pubkey.toLowerCase());
        })
        .catch((error) => console.debug("[accounts] watch list failed", error));
    };
    reload();
    const unlisten = listen(ACCOUNTS_CHANGED_EVENT, reload);
    return () => {
      cancelled = true;
      void unlisten.then((stop) => stop());
    };
  }, [enabled, openOrigin]);

  // With a built-in server, accounts on any other server are never
  // connected to, not even read-only.
  const builtInServer = useBuiltInServer();
  const targets = React.useMemo(
    () =>
      enabled && openPubkey
        ? watchTargets(accounts, communities, {
            origin: openOrigin,
            pubkey: openPubkey,
          }).filter((target) => isAllowedServer(target.relayUrl, builtInServer))
        : [],
    [enabled, accounts, builtInServer, communities, openOrigin, openPubkey],
  );

  React.useEffect(() => {
    const running = watchers.current;
    const wanted = new Map(targets.map((target) => [target.key, target]));
    for (const [key, watcher] of running) {
      const target = wanted.get(key);
      if (!target || target.relayUrl !== watcher.relayUrl) {
        watcher.stop();
        running.delete(key);
      }
    }
    for (const target of targets) {
      if (running.has(target.key)) continue;
      const watcher = new InactiveAccountWatcher(
        target.key,
        target.relayUrl,
        target.account.pubkey,
        watcherDeps(target),
      );
      running.set(target.key, watcher);
      watcher.start();
    }
  }, [targets]);

  React.useEffect(() => {
    const running = watchers.current;
    const uninstall = installRelayResumeTriggers(() => {
      for (const watcher of running.values()) watcher.resume();
    });
    return () => {
      uninstall();
      for (const watcher of running.values()) watcher.stop();
      running.clear();
    };
  }, []);
}
