import type { Channel, ManagedAgent, RelayEvent } from "@/shared/api/types";
import { KIND_STREAM_MESSAGE } from "@/shared/constants/kinds";
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

/** The owner's Stop, which a later `!start` overrides and vice versa. */
export const REMOTE_STOP_COMMAND = "!shutdown";

const SEEN_LIMIT = 500;

type CommandEvent = Pick<
  RelayEvent,
  "content" | "created_at" | "id" | "kind" | "pubkey" | "tags"
>;

/**
 * The owner's Start and Stop commands seen so far, per agent. A command is
 * acted on once, and a Start only while it is still the agent's latest
 * command: a replayed `!start` followed by a `!shutdown` does not restart
 * the agent.
 */
export class RemoteCommandLedger {
  private readonly seen: string[] = [];
  private readonly seenSet = new Set<string>();
  private readonly latest = new Map<string, { at: number; id: string }>();

  /** Records `event`; false when it was already recorded. */
  record(event: CommandEvent): boolean {
    if (this.seenSet.has(event.id)) return false;
    this.seen.push(event.id);
    this.seenSet.add(event.id);
    if (this.seen.length > SEEN_LIMIT) {
      const dropped = this.seen.shift();
      if (dropped) this.seenSet.delete(dropped);
    }
    const content = event.content.trim();
    if (
      event.kind !== KIND_STREAM_MESSAGE ||
      (content !== REMOTE_START_COMMAND && content !== REMOTE_STOP_COMMAND)
    ) {
      return true;
    }
    for (const pubkey of mentionedPubkeys(event)) {
      const current = this.latest.get(pubkey);
      if (
        !current ||
        event.created_at > current.at ||
        (event.created_at === current.at && event.id > current.id)
      ) {
        this.latest.set(pubkey, { at: event.created_at, id: event.id });
      }
    }
    return true;
  }

  /** Whether `event` is still the latest command for `agentPubkey`. */
  isLatest(event: Pick<RelayEvent, "id">, agentPubkey: string): boolean {
    return this.latest.get(normalizePubkey(agentPubkey))?.id === event.id;
  }
}

function mentionedPubkeys(event: Pick<RelayEvent, "tags">): Set<string> {
  return new Set(
    event.tags
      .filter((tag) => tag[0] === "p" && tag[1])
      .map((tag) => normalizePubkey(tag[1])),
  );
}

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
    localAgents: readonly (Pick<ManagedAgent, "backend" | "pubkey" | "status"> &
      Partial<Pick<ManagedAgent, "ownerPubkey">>)[];
    nowSeconds: number;
    ownerPubkey: string;
  },
): string[] {
  // kind:9 only, the kind the agent's harness consumes an owner `!start`
  // in, so a start command never reaches a running agent as a prompt.
  if (event.kind !== KIND_STREAM_MESSAGE) return [];
  if (event.content.trim() !== REMOTE_START_COMMAND) return [];
  if (normalizePubkey(event.pubkey) !== normalizePubkey(ownerPubkey)) return [];
  if (nowSeconds - event.created_at > REMOTE_START_MAX_AGE_SECONDS) return [];
  const mentioned = mentionedPubkeys(event);
  return localAgents
    .filter(
      (agent) =>
        mentioned.has(normalizePubkey(agent.pubkey)) &&
        // Another account's agent on this computer is not this owner's.
        (!agent.ownerPubkey ||
          normalizePubkey(agent.ownerPubkey) ===
            normalizePubkey(ownerPubkey)) &&
        agent.backend.type === "local" &&
        agent.status !== "running",
    )
    .map((agent) => agent.pubkey);
}

/**
 * The owner's DMs with this computer's local agents: where another of the
 * owner's devices sends `!start` (and `!shutdown`).
 */
export function agentDmChannelIds(
  channels: readonly Pick<
    Channel,
    "channelType" | "id" | "participantPubkeys"
  >[],
  agentPubkeys: readonly string[],
  ownerPubkey: string,
): string[] {
  const owner = normalizePubkey(ownerPubkey);
  const agents = new Set(agentPubkeys.map(normalizePubkey));
  return channels
    .filter((channel) => {
      if (channel.channelType !== "dm") return false;
      const members = new Set(channel.participantPubkeys.map(normalizePubkey));
      if (members.size !== 2 || !members.has(owner)) return false;
      return [...members].some((member) => agents.has(member));
    })
    .map((channel) => channel.id)
    .sort();
}
