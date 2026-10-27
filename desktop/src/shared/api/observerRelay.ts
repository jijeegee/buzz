import { t } from "@/shared/i18n";
import { buildObserverControlEvent } from "@/shared/api/tauriObserver";
import type { RelayEvent } from "@/shared/api/types";
import { buildObserverLiveFilter } from "@/shared/api/observerLiveFilter";
import { relayClient } from "./relayClient";

export function subscribeToAgentObserverFrames(
  ownerPubkey: string,
  onEvent: (event: RelayEvent) => void,
) {
  // Ephemeral control results must not wait behind cold channel history.
  return relayClient.subscribeInteractive(
    buildObserverLiveFilter(ownerPubkey, Math.floor(Date.now() / 1_000)),
    onEvent,
  );
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
