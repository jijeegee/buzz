import { captureRelayRemovals } from "@/features/agents/managedAgentRelayCleanup";
import { openDm, sendChannelMessage } from "@/shared/api/tauri";
import type { Channel, ManagedAgent, RelayAgent } from "@/shared/api/types";
import type { AgentAvailabilityReader } from "./useAgentAvailability";
import { normalizePubkey } from "@/shared/lib/pubkey";
import { t } from "@/shared/i18n";

type DeleteManagedAgentInput = {
  pubkey: string;
  forceRemoteDelete?: boolean;
};

type StartManagedAgent = (pubkey: string) => Promise<unknown>;
type StopManagedAgent = (pubkey: string) => Promise<unknown>;
type DeleteManagedAgent = (input: DeleteManagedAgentInput) => Promise<unknown>;

type ManagedAgentChannelContext = {
  channels: readonly Channel[];
  preferredChannelId?: string | null;
  relayAgents: readonly RelayAgent[];
};

type ManagedAgentActionContext = ManagedAgentChannelContext & {
  getAvailability: AgentAvailabilityReader;
};

export type ManagedAgentActionResult = {
  cancelled?: boolean;
  noticeMessage?: string;
};

/** Lifecycle action routing only; deployed is a retained receipt, not presence. */
export function isManagedAgentActive(agent: Pick<ManagedAgent, "status">) {
  return agent.status === "running" || agent.status === "deployed";
}

export function getManagedAgentPrimaryActionLabel(agent: ManagedAgent) {
  if (agent.backend.type === "provider") {
    return isManagedAgentActive(agent)
      ? t("agents.control.shutdown")
      : t("agents.control.deploy");
  }

  if (isManagedAgentActive(agent)) {
    return t("agents.control.stop");
  }

  return t("agents.control.start");
}

export function resolveManagedAgentChannelId(
  agent: Pick<ManagedAgent, "pubkey">,
  context: ManagedAgentChannelContext,
) {
  if (context.preferredChannelId) {
    return context.preferredChannelId;
  }

  const relayAgent = context.relayAgents.find(
    (candidate) =>
      normalizePubkey(candidate.pubkey) === normalizePubkey(agent.pubkey),
  );

  if (relayAgent?.channelIds?.length) {
    return relayAgent.channelIds[0];
  }

  const channelName = relayAgent?.channels?.[0];
  if (!channelName) {
    return null;
  }

  const matches = context.channels.filter(
    (channel) => channel.name === channelName,
  );
  return matches.length === 1 ? matches[0].id : null;
}

export async function startManagedAgentWithRules({
  agent,
  startManagedAgent,
}: {
  agent: ManagedAgent;
  startManagedAgent: StartManagedAgent;
}) {
  // Relay-mesh agents are no longer blocked here: the backend start preflight
  // (ensure_relay_mesh_for_record) re-resolves a live serve target and dials
  // it, failing with an actionable error when no peer serves the model.
  await startManagedAgent(agent.pubkey);
}

export async function respawnManagedAgentWithRules({
  agent,
  relayUrl,
  startManagedAgent,
  stopManagedAgent,
  onStopped,
}: {
  agent: ManagedAgent;
  /** The active community's relay: the workspace pair the native stop and
   * start target. A removal of it during the stop cancels the start. */
  relayUrl: string | undefined;
  startManagedAgent: StartManagedAgent;
  stopManagedAgent: StopManagedAgent;
  /** Called after a successful stop and before start begins — use this to
   * clear stale working badges at the right boundary. */
  onStopped?: () => void;
}) {
  if (agent.backend.type === "local" && isManagedAgentActive(agent)) {
    const assertRelayNotRemoved = relayUrl
      ? captureRelayRemovals(relayUrl)
      : () => {};
    await stopManagedAgent(agent.pubkey);
    onStopped?.();
    assertRelayNotRemoved();
  }

  await startManagedAgent(agent.pubkey);
}

export async function stopManagedAgentWithRules({
  agent,
  channels,
  preferredChannelId,
  relayAgents,
  stopManagedAgent,
}: {
  agent: ManagedAgent;
  stopManagedAgent: StopManagedAgent;
} & ManagedAgentChannelContext): Promise<ManagedAgentActionResult> {
  if (agent.backend.type === "provider") {
    const channelId = resolveManagedAgentChannelId(agent, {
      channels,
      preferredChannelId,
      relayAgents,
    });
    if (!channelId) {
      throw new Error(t("agents.control.stopNoGroup"));
    }

    await sendChannelMessage(channelId, "!shutdown", undefined, undefined, [
      agent.pubkey,
    ]);
    return {
      noticeMessage: t("agents.control.shutdownRequested"),
    };
  }

  await stopManagedAgent(agent.pubkey);
  return {};
}

/**
 * The owner's DM with an agent, opened when there is none. Commands to an
 * agent on another computer go there, so no one else sees them and an agent
 * in no channel can still be reached.
 */
async function ownerCommandChannelId(pubkey: string): Promise<string> {
  return (await openDm({ pubkeys: [pubkey] })).id;
}

/**
 * Ask an agent running on another of the owner's computers to stop: the
 * owner's `!shutdown` in their DM with it makes its harness exit, and the
 * hosting desktop leaves a cleanly exited agent stopped.
 */
export async function stopRemoteOwnedAgent({
  pubkey,
}: {
  pubkey: string;
}): Promise<ManagedAgentActionResult> {
  const channelId = await ownerCommandChannelId(pubkey);
  await sendChannelMessage(channelId, "!shutdown", undefined, undefined, [
    pubkey,
  ]);
  return {
    noticeMessage: t("agents.control.stopRequested"),
  };
}

/**
 * Ask the computer an agent runs on to start it: the owner's `!start` in
 * their DM with it reaches that computer's Kuzz app (`useRemoteStartListener`
 * listens to the owner's messages in any channel), which starts the stopped
 * agent. Nothing happens while that app is closed.
 */
export async function startRemoteOwnedAgent({
  pubkey,
  hostName,
}: {
  pubkey: string;
  /** The hosting computer's name, when known. */
  hostName?: string | null;
}): Promise<ManagedAgentActionResult> {
  const channelId = await ownerCommandChannelId(pubkey);
  await sendChannelMessage(channelId, "!start", undefined, undefined, [pubkey]);
  return {
    noticeMessage: hostName?.trim()
      ? t("agents.control.startRequestedOn", { host: hostName.trim() })
      : t("agents.control.startRequested"),
  };
}

export async function deleteManagedAgentWithRules({
  agent,
  channels,
  deleteManagedAgent,
  preferredChannelId,
  getAvailability,
  relayAgents,
  skipRemoteDeleteConfirm = false,
}: {
  agent: ManagedAgent;
  deleteManagedAgent: DeleteManagedAgent;
  skipRemoteDeleteConfirm?: boolean;
} & ManagedAgentActionContext): Promise<ManagedAgentActionResult> {
  if (agent.backend.type === "provider" && agent.backendAgentId) {
    const availability = getAvailability(agent.pubkey);
    const channelId = resolveManagedAgentChannelId(agent, {
      channels,
      preferredChannelId,
      relayAgents,
    });

    if (channelId) {
      // Only established Offline preserves the intentional no-request path.
      // Unknown is not evidence that shutdown can safely be skipped.
      if (availability !== "offline") {
        await sendChannelMessage(channelId, "!shutdown", undefined, undefined, [
          agent.pubkey,
        ]);

        if (!skipRemoteDeleteConfirm) {
          const confirmed = window.confirm(
            availability === undefined
              ? t("agents.control.deleteConfirmUnknown")
              : t("agents.control.deleteConfirmRunning"),
          );
          if (!confirmed) {
            return { cancelled: true };
          }
        }
      } else {
        if (!skipRemoteDeleteConfirm) {
          const confirmed = window.confirm(
            t("agents.control.deleteConfirmOffline"),
          );
          if (!confirmed) {
            return { cancelled: true };
          }
        }
      }
    } else {
      if (!skipRemoteDeleteConfirm) {
        const confirmed = window.confirm(
          t("agents.control.deleteConfirmNoGroup"),
        );
        if (!confirmed) {
          return { cancelled: true };
        }
      }
    }
  }

  const isDeployedRemote =
    agent.backend.type === "provider" && agent.backendAgentId;
  await deleteManagedAgent({
    pubkey: agent.pubkey,
    forceRemoteDelete: isDeployedRemote ? true : undefined,
  });

  return {};
}
