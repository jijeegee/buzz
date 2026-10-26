import * as React from "react";

import { useAppShellRoom } from "@/app/AppShellContext";

type ArrivalEvent = { id: string; pubkey: string; created_at: number };

/**
 * Ids of `events` from others not yet seen for this context, dated at or
 * before `readAt`: they arrived after the read was sent but did not move it.
 * Seeds `seen` with every id so each event is judged once. Pure.
 */
export function lateArrivals(
  events: readonly ArrivalEvent[],
  seen: Set<string>,
  readAt: number,
  selfPubkey: string | undefined,
): string[] {
  const self = selfPubkey?.toLowerCase();
  const late: string[] = [];
  for (const event of events) {
    if (seen.has(event.id)) continue;
    seen.add(event.id);
    if (event.pubkey.toLowerCase() !== self && event.created_at <= readAt) {
      late.push(event.id);
    }
  }
  return late;
}

/**
 * The relay counts unread by arrival order, the read sent for an open room or
 * thread is a time. A message from someone else that arrives while it is
 * open but is dated at or before that time (a device clock behind, a send
 * signed before it reached the relay) leaves the read unchanged, so nothing
 * would clear its unread count. Send the same read again when one arrives.
 * The first render of a context only records what is already there.
 */
export function useLateArrivalRead(
  contextId: string | null,
  readAt: number | null,
  events: readonly ArrivalEvent[],
  selfPubkey: string | undefined,
  enabled = true,
) {
  const { resendRead } = useAppShellRoom();
  const seenRef = React.useRef<{ contextId: string | null; ids: Set<string> }>({
    contextId: null,
    ids: new Set(),
  });
  React.useEffect(() => {
    const seen = seenRef.current;
    if (seen.contextId !== contextId) {
      seenRef.current = { contextId, ids: new Set(events.map((e) => e.id)) };
      return;
    }
    if (!contextId || readAt === null || !enabled) return;
    if (lateArrivals(events, seen.ids, readAt, selfPubkey).length > 0) {
      resendRead(contextId, readAt);
    }
  }, [contextId, enabled, events, readAt, resendRead, selfPubkey]);
}
