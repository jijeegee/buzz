import type { ManagedAgent, RelayEvent } from "@/shared/api/types";
import {
  KIND_STREAM_MESSAGE,
  KIND_STREAM_MESSAGE_V2,
} from "@/shared/constants/kinds";
import { normalizePubkey } from "@/shared/lib/pubkey";

/**
 * The owner's command, from another of their devices, to start an agent
 * that runs on this computer: a channel message `!start` that mentions the
 * agent, like `!shutdown` for Stop. The agent is not running, so the desktop
 * that hosts it acts on it; a running agent's harness ignores it.
 */
export const REMOTE_START_COMMAND = "!start";

/**
 * A command older than this is not acted on: it was meant for a moment
 * that has passed (this computer was off or the app closed).
 */
export const REMOTE_START_MAX_AGE_SECONDS = 120;

/** The pubkeys of this computer's stopped local agents `event` asks to start. */
export function remoteStartTargets(
  event: Pick<
    RelayEvent,
    "content" | "created_at" | "kind" | "pubkey" | "tags"
  >,
  {
    localAgents,
    nowSeconds,
    ownerPubkey,
  }: {
    localAgents: readonly Pick<ManagedAgent, "backend" | "pubkey" | "status">[];
    nowSeconds: number;
    ownerPubkey: string;
  },
): string[] {
  if (
    event.kind !== KIND_STREAM_MESSAGE &&
    event.kind !== KIND_STREAM_MESSAGE_V2
  ) {
    return [];
  }
  if (event.content.trim() !== REMOTE_START_COMMAND) return [];
  if (normalizePubkey(event.pubkey) !== normalizePubkey(ownerPubkey)) return [];
  if (nowSeconds - event.created_at > REMOTE_START_MAX_AGE_SECONDS) return [];
  const mentioned = new Set(
    event.tags
      .filter((tag) => tag[0] === "p" && tag[1])
      .map((tag) => normalizePubkey(tag[1])),
  );
  return localAgents
    .filter(
      (agent) =>
        mentioned.has(normalizePubkey(agent.pubkey)) &&
        agent.backend.type === "local" &&
        agent.status !== "running",
    )
    .map((agent) => agent.pubkey);
}
