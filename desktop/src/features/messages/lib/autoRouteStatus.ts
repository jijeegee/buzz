import * as React from "react";

/**
 * Smart routing's local state for a message the owner just sent, keyed by
 * event id. Routing runs after the publish, so the sent row shows it in its
 * trailing auto-route spot: "Routing…", then "→ Delivered to Name" (also
 * carried by the message's `auto-route` tags once the delivery edit lands),
 * or "Not delivered". Community-scoped: reset by `resetCommunityState()`.
 */
export type AutoRouteStatus =
  | { status: "routing" }
  | { status: "delivered"; pubkeys: string[] }
  | { status: "failed" };

type Entry = AutoRouteStatus & { generation: number };

const entries = new Map<string, Entry>();
const listeners = new Set<() => void>();
let nextGeneration = 0;

function emit() {
  for (const listener of listeners) listener();
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** Start routing `eventId`; returns the generation that fences its result. */
export function beginAutoRoute(eventId: string): number {
  nextGeneration += 1;
  entries.set(eventId, { status: "routing", generation: nextGeneration });
  emit();
  return nextGeneration;
}

/**
 * Whether `generation` is still the message's routing attempt. An edit or
 * delete of the message (or a community switch) since it began fences it
 * out, so a late pick is never delivered to a stale message.
 */
export function isAutoRouteCurrent(eventId: string, generation: number) {
  return entries.get(eventId)?.generation === generation;
}

/** Record the outcome; `null` clears the row (nobody fit). Fenced. */
export function settleAutoRoute(
  eventId: string,
  generation: number,
  outcome: AutoRouteStatus | null,
) {
  if (!isAutoRouteCurrent(eventId, generation)) return;
  if (outcome === null) entries.delete(eventId);
  else entries.set(eventId, { ...outcome, generation });
  emit();
}

/** The owner edited or deleted the message: abandon any routing in flight. */
export function cancelAutoRoute(eventId: string) {
  if (entries.delete(eventId)) emit();
}

export function resetAutoRouteStatus() {
  entries.clear();
  emit();
}

export function getAutoRouteStatus(
  eventId: string | undefined,
): AutoRouteStatus | null {
  return eventId ? (entries.get(eventId) ?? null) : null;
}

export function useAutoRouteStatus(
  eventId: string | undefined,
): AutoRouteStatus | null {
  return React.useSyncExternalStore(subscribe, () =>
    getAutoRouteStatus(eventId),
  );
}
