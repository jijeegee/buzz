import * as React from "react";

import {
  fetchQuotePreviewFallback,
  type QuotePreviewDeps,
  type QuotePreviewFallback,
  quotePreviewCacheKey,
} from "@/features/messages/lib/quotePreviewFallback";
import { getQuotePreview } from "@/shared/api/tauriEvents";
import { getUserProfile } from "@/shared/api/tauriProfiles";

const DEPS: QuotePreviewDeps = {
  fetchPreview: ({ channelId, quotedId, quotingId }) =>
    getQuotePreview(channelId, quotedId, quotingId),
  fetchAuthorName: async (pubkey) => {
    const profile = await getUserProfile(pubkey);
    return profile.displayName?.trim() || profile.nip05Handle?.trim() || null;
  },
};

/**
 * Preview of a quoted message sent before the reader joined. Requested only
 * when `enabled` (the normal lookup found nothing); `null` while loading.
 */
export function useQuotePreviewFallback(
  channelId: string,
  quotedId: string,
  quotingId: string | null | undefined,
  enabled: boolean,
): QuotePreviewFallback | null {
  const request = React.useMemo(
    () =>
      quotingId
        ? { channelId, quotedId, quotingId: quotingId.toLowerCase() }
        : null,
    [channelId, quotedId, quotingId],
  );
  const key = request ? quotePreviewCacheKey(request) : null;
  const [resolved, setResolved] = React.useState<{
    key: string;
    value: QuotePreviewFallback;
  } | null>(null);

  React.useEffect(() => {
    if (!enabled || !request || !key) return;
    let current = true;
    void fetchQuotePreviewFallback(request, DEPS).then((value) => {
      if (current) setResolved({ key, value });
    });
    return () => {
      current = false;
    };
  }, [enabled, key, request]);

  if (!enabled) return null;
  if (!request) return { kind: "none" };
  return resolved?.key === key ? resolved.value : null;
}
