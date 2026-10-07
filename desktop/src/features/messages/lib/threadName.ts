import type { RelayEvent } from "@/shared/api/types";
import { KIND_THREAD_NAME } from "@/shared/constants/kinds";

export const MAX_THREAD_NAME_WEIGHT = 40;

export function threadNameWeight(name: string): number {
  return Array.from(name).reduce(
    (weight, char) => weight + (char.charCodeAt(0) <= 127 ? 1 : 2),
    0,
  );
}

export function isValidThreadName(name: string): boolean {
  return (
    name === name.trim() &&
    !/[\p{Cc}\p{Zl}\p{Zp}]/u.test(name) &&
    threadNameWeight(name) <= MAX_THREAD_NAME_WEIGHT
  );
}

export function selectThreadName(
  current: RelayEvent | null,
  incoming: RelayEvent,
  channelId: string,
  threadId: string,
): RelayEvent | null {
  if (
    incoming.kind !== KIND_THREAD_NAME ||
    !isValidThreadName(incoming.content) ||
    !incoming.tags.some((tag) => tag[0] === "h" && tag[1] === channelId) ||
    !incoming.tags.some((tag) => tag[0] === "e" && tag[1] === threadId)
  )
    return current;
  // Match the relay's newest-first query order, including same-second writes.
  return !current ||
    incoming.created_at > current.created_at ||
    (incoming.created_at === current.created_at && incoming.id < current.id)
    ? incoming
    : current;
}
