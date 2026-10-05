import type { RouterPriorDelivery } from "@/shared/api/tauriMessageRouting";

/**
 * Smart routing's local record of what this desktop delivered, kept for
 * {@link AUTO_ROUTE_LEDGER_MS}. It makes delivery exactly-once (a message
 * already here is never routed again) and gives the router PRIOR: what a
 * follow-up may continue, amend, or cancel, and which thread that work
 * lives in. Community-scoped: reset by `resetCommunityState()`.
 */
export const AUTO_ROUTE_LEDGER_MS = 30 * 60_000;

export type AutoRouteDelivery = {
  messageId: string;
  channelId: string;
  agents: string[];
  /** The thread the agents answer in: the message's root, or itself. */
  threadRootId: string;
  text: string;
  at: number;
};

const deliveries = new Map<string, AutoRouteDelivery>();

function prune(now: number) {
  for (const [id, delivery] of deliveries) {
    if (now - delivery.at > AUTO_ROUTE_LEDGER_MS) deliveries.delete(id);
  }
}

export function recordAutoRouteDelivery(delivery: AutoRouteDelivery) {
  prune(delivery.at);
  deliveries.set(delivery.messageId, delivery);
}

export function getAutoRouteDelivery(
  messageId: string,
  now = Date.now(),
): AutoRouteDelivery | null {
  prune(now);
  return deliveries.get(messageId) ?? null;
}

export function wasAutoRouteDelivered(messageId: string, now = Date.now()) {
  return getAutoRouteDelivery(messageId, now) !== null;
}

/** The channel's deliveries, oldest first, as the router's PRIOR. */
export function getAutoRoutePrior(
  channelId: string,
  now = Date.now(),
): RouterPriorDelivery[] {
  prune(now);
  return [...deliveries.values()]
    .filter((delivery) => delivery.channelId === channelId)
    .sort((a, b) => a.at - b.at)
    .map(({ messageId, agents, text }) => ({ id: messageId, agents, text }));
}

export function resetAutoRouteLedger() {
  deliveries.clear();
}
