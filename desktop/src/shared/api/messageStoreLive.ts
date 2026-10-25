import type { RelayEvent } from "@/shared/api/types";
import { getIdentity } from "@/shared/api/tauriIdentity";
import { CHANNEL_EVENT_KINDS } from "@/shared/constants/kinds";

/**
 * The `connect` config that lets the native socket hand the room events it
 * receives to this account's local message store. Only the main relay
 * session passes it; read-only account watchers connect without, so nothing
 * they receive is stored. Without an identity the socket is simply not tapped.
 */
export async function messageStoreSocketConfig(
  relayUrl: string,
): Promise<{ storeScope?: { pubkey: string; relayUrl: string } }> {
  try {
    const { pubkey } = await getIdentity();
    return pubkey ? { storeScope: { pubkey, relayUrl } } : {};
  } catch {
    return {};
  }
}

const UI_KINDS = new Set<number>(CHANNEL_EVENT_KINDS);

/**
 * Live room subscriptions request CHANNEL_SYNC_KINDS so the local store's
 * live segments cover every synced kind; their consumers keep receiving only
 * the kinds they handled before (plus `extraKinds`).
 */
export function uiKindsOnly(
  onEvent: (event: RelayEvent) => void,
  ...extraKinds: number[]
): (event: RelayEvent) => void {
  return (event) => {
    if (UI_KINDS.has(event.kind) || extraKinds.includes(event.kind)) {
      onEvent(event);
    }
  };
}
