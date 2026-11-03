import { t } from "@/shared/i18n";
import { buildObserverControlEvent } from "@/shared/api/tauriObserver";
import type { RelayEvent } from "@/shared/api/types";
import { buildObserverLiveFilter } from "@/shared/api/observerLiveFilter";
import { meLive } from "@/shared/api/relayMeLive";
import { relayClient } from "./relayClient";

export function subscribeToAgentObserverFrames(
  ownerPubkey: string,
  onEvent: (event: RelayEvent) => void,
) {
  // Rides the addressed account bundle (docs/message-sync.md#agent-activity):
  // a live-only REQ, sent ahead of cold channel history.
  return meLive.subscribe(buildObserverLiveFilter(ownerPubkey), onEvent);
}

export async function sendAgentObserverControl(
  agentPubkey: string,
  payload: unknown,
) {
  await relayClient.preconnect();
  const event = await buildObserverControlEvent({ agentPubkey, payload });
  await relayClient.publishEvent(
    event,
    t("ui.agentControl.sendTimedOut"),
    t("ui.agentControl.sendFailed"),
  );
}
