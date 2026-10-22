import * as React from "react";

import { relayClient } from "@/shared/api/relayClient";
import {
  nip44DecryptFromSelf,
  nip44EncryptToSelf,
  signRelayEvent,
} from "@/shared/api/tauri";
import type { RelayEvent } from "@/shared/api/types";
import { KIND_CHANNEL_NAMES } from "@/shared/constants/kinds";
import {
  getChatPrivateNames,
  parsePrivateNamesPayload,
  readCachedPrivateNames,
  serializePrivateNames,
  setChatPrivateNames,
  withPrivateName,
  writeCachedPrivateNames,
} from "./lib/chatPrivateNames";
import { t } from "@/shared/i18n";

const D_TAG = "channel-names";

type Head = { createdAt: number; eventId: string };

function isNewer(event: Pick<RelayEvent, "created_at" | "id">, head: Head) {
  return (
    event.created_at > head.createdAt ||
    (event.created_at === head.createdAt && event.id > head.eventId)
  );
}

async function decryptNames(event: RelayEvent) {
  const plaintext = await nip44DecryptFromSelf(event.content);
  return parsePrivateNamesPayload(JSON.parse(plaintext));
}

/**
 * Keep this account's private chat names in sync (kind:30078
 * `d=channel-names`). The whole blob is last-write-wins; an edit first reads
 * the newest blob so another device's names are kept, then writes one event.
 * Returns the edit function; failures reject so the caller can say so.
 */
export function useChatPrivateNamesSync(
  pubkey: string | undefined,
  relayUrl: string | undefined,
) {
  const headRef = React.useRef<Head>({ createdAt: 0, eventId: "" });
  const scopeRef = React.useRef(0);

  const fetchLatest = React.useCallback(async () => {
    if (!pubkey) return null;
    const events = await relayClient.fetchEvents({
      kinds: [KIND_CHANNEL_NAMES],
      authors: [pubkey],
      "#d": [D_TAG],
      limit: 1,
    });
    const event = events.find((candidate) => candidate.pubkey === pubkey);
    return event ?? null;
  }, [pubkey]);

  const applyRemote = React.useCallback(
    async (event: RelayEvent, scope: number) => {
      if (!pubkey || !isNewer(event, headRef.current)) return;
      const names = await decryptNames(event);
      if (scope !== scopeRef.current || !isNewer(event, headRef.current)) {
        return;
      }
      headRef.current = { createdAt: event.created_at, eventId: event.id };
      setChatPrivateNames(names);
      writeCachedPrivateNames(pubkey, names);
    },
    [pubkey],
  );

  // biome-ignore lint/correctness/useExhaustiveDependencies: relayUrl rebinds the account scope on a community switch
  React.useEffect(() => {
    const scope = ++scopeRef.current;
    headRef.current = { createdAt: 0, eventId: "" };
    if (!pubkey) {
      setChatPrivateNames(new Map());
      return;
    }
    setChatPrivateNames(readCachedPrivateNames(pubkey));
    const refresh = () => {
      void fetchLatest()
        .then((event) => (event ? applyRemote(event, scope) : undefined))
        .catch((error) =>
          console.warn("[chatPrivateNames] fetch failed:", error),
        );
    };
    refresh();
    let unsubscribe: (() => Promise<void>) | null = null;
    let cancelled = false;
    void relayClient
      .subscribeLive(
        {
          kinds: [KIND_CHANNEL_NAMES],
          authors: [pubkey],
          "#d": [D_TAG],
          limit: 0,
        },
        (event: RelayEvent) => {
          if (event.pubkey !== pubkey) return;
          void applyRemote(event, scope).catch(() => {});
        },
      )
      .then((dispose) => {
        if (cancelled) void dispose();
        else unsubscribe = dispose;
      })
      .catch(() => {});
    const stopReconnects = relayClient.subscribeToReconnects(refresh);
    return () => {
      cancelled = true;
      if (unsubscribe) void unsubscribe();
      stopReconnects();
    };
  }, [pubkey, relayUrl, applyRemote, fetchLatest]);

  return React.useCallback(
    async (channelId: string, name: string | null) => {
      if (!pubkey) throw new Error(t("channels.chatInfo.notSignedIn"));
      const scope = scopeRef.current;
      const previous = getChatPrivateNames();
      setChatPrivateNames(withPrivateName(previous, channelId, name));
      try {
        const latest = await fetchLatest();
        if (latest) await applyRemote(latest, scope);
        if (scope !== scopeRef.current) return;
        const next = withPrivateName(getChatPrivateNames(), channelId, name);
        const content = await nip44EncryptToSelf(serializePrivateNames(next));
        const event = await signRelayEvent({
          kind: KIND_CHANNEL_NAMES,
          content,
          createdAt: Math.max(
            Math.floor(Date.now() / 1_000),
            headRef.current.createdAt + 1,
          ),
          tags: [["d", D_TAG]],
        });
        if (scope !== scopeRef.current) return;
        await relayClient.publishEvent(
          event,
          t("channels.chatInfo.saveTimedOut"),
          t("channels.chatInfo.saveNameFailed"),
          () => scope === scopeRef.current,
        );
        headRef.current = { createdAt: event.created_at, eventId: event.id };
        setChatPrivateNames(next);
        writeCachedPrivateNames(pubkey, next);
      } catch (error) {
        if (scope === scopeRef.current) setChatPrivateNames(previous);
        throw error;
      }
    },
    [pubkey, applyRemote, fetchLatest],
  );
}
