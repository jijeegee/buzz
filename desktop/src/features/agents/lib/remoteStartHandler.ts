import { toast } from "sonner";

import { listAllManagedAgents } from "@/shared/api/tauri";
import { startManagedAgent } from "@/shared/api/tauriManagedAgents";
import type { RelayEvent } from "@/shared/api/types";
import { normalizePubkey } from "@/shared/lib/pubkey";
import {
  REMOTE_START_COMMAND,
  RemoteCommandLedger,
  remoteStartTargets,
} from "./remoteStartCommand";
import { t } from "@/shared/i18n";

/**
 * How long a `!start` waits before it is acted on, so a later `!shutdown`
 * replayed in the same batch (a reconnect, a newly seen DM) cancels it.
 */
export const REMOTE_START_SETTLE_MS = 1_500;

/** Shared by every account's listener: one event is acted on once. */
const ledger = new RemoteCommandLedger();

export type RemoteStartScope = {
  ownerPubkey: string;
  relayUrl: string;
  /** Whether `ownerPubkey` is the open account. Only then is the start
   * bound to it, and only then do agents with no recorded owner count. */
  openAccount: boolean;
  onStarted?: () => void;
};

/**
 * Handles one of the owner's own channel messages: records Start / Stop
 * commands and starts this computer's stopped agents a fresh `!start`
 * mentions, unless a later command for the agent arrives first.
 */
export function handleRemoteCommand(
  event: RelayEvent,
  scope: RemoteStartScope,
): void {
  if (normalizePubkey(event.pubkey) !== normalizePubkey(scope.ownerPubkey)) {
    return;
  }
  if (!event.content.trim().startsWith("!")) return;
  if (!ledger.record(event)) return;
  if (event.content.trim() !== REMOTE_START_COMMAND) return;
  setTimeout(() => {
    startTargets(event, scope).catch((error) => {
      console.error("Remote start check failed", error);
    });
  }, REMOTE_START_SETTLE_MS);
}

async function startTargets(event: RelayEvent, scope: RemoteStartScope) {
  const owner = normalizePubkey(scope.ownerPubkey);
  const agents = (await listAllManagedAgents()).filter((agent) =>
    agent.ownerPubkey
      ? normalizePubkey(agent.ownerPubkey) === owner
      : scope.openAccount,
  );
  const targets = remoteStartTargets(event, {
    localAgents: agents,
    nowSeconds: Math.floor(Date.now() / 1_000),
    ownerPubkey: scope.ownerPubkey,
  }).filter((pubkey) => ledger.isLatest(event, pubkey));

  let started = false;
  for (const pubkey of targets) {
    try {
      // For the open account, scoped like a mention wake: a community or
      // account switch since the command arrived fails closed. Another
      // account's agent runs for that account whatever is open.
      const agent = await startManagedAgent(
        pubkey,
        scope.openAccount
          ? { expectedRelayUrl: scope.relayUrl, expectedSignerPubkey: owner }
          : undefined,
      );
      toast.success(t("agents.remoteStart.started", { name: agent.name }));
      started = true;
    } catch (error) {
      console.error("Remote start failed", pubkey, error);
    }
  }
  if (started) scope.onStarted?.();
}
