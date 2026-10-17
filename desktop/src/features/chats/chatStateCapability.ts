import { isChatStateUnavailable } from "@/shared/api/chatState";

/**
 * Whether the relay keeps chat state for one relay + pubkey. "unknown" (not
 * probed yet, or the probe failed for another reason) behaves like "off".
 */
export type ChatStateCapability = "unknown" | "on" | "off";

const states = new Map<string, ChatStateCapability>();
const generations = new Map<string, number>();
const listeners = new Set<() => void>();

export function chatStateCapabilityKey(
  relayUrl: string | undefined,
  pubkey: string | undefined,
): string | null {
  if (!relayUrl || !pubkey) return null;
  return `${relayUrl}|${pubkey.toLowerCase()}`;
}

export function getChatStateCapability(
  key: string | null,
): ChatStateCapability {
  return key === null ? "unknown" : (states.get(key) ?? "unknown");
}

export function subscribeChatStateCapability(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** Set a capability; also fences any probe still in flight for that key. */
export function setChatStateCapability(
  key: string,
  value: ChatStateCapability,
): void {
  generations.set(key, (generations.get(key) ?? 0) + 1);
  if (states.get(key) === value) return;
  states.set(key, value);
  for (const listener of listeners) listener();
}

/**
 * Ask the relay once. Success turns the capability on, "unavailable" off;
 * other failures (offline, timeout) leave it unchanged. A probe that lands
 * after a newer probe or an explicit set is ignored.
 */
export async function probeChatStateCapability(
  key: string,
  probe: () => Promise<unknown>,
): Promise<ChatStateCapability> {
  const generation = (generations.get(key) ?? 0) + 1;
  generations.set(key, generation);
  let next: ChatStateCapability | null = null;
  try {
    await probe();
    next = "on";
  } catch (error) {
    if (isChatStateUnavailable(error)) next = "off";
  }
  if (next !== null && generations.get(key) === generation) {
    setChatStateCapability(key, next);
  }
  return getChatStateCapability(key);
}

/** Test seam: forget every capability. */
export function resetChatStateCapabilities(): void {
  states.clear();
  generations.clear();
  for (const listener of listeners) listener();
}
