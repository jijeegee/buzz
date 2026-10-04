import { isDefaultAiEligible } from "@/features/agents/lib/defaultAi";
import type { ManagedAgent } from "@/shared/api/types";
import type {
  AgentRoutingTransition,
  ChannelRoutingMode,
  ChannelRoutingStatus,
  RoutingRole,
} from "@/shared/api/tauriChannelRouting";

/**
 * Pure projections of the Rust-owned channel routing status for the Agents
 * page card and the Settings summary. Rust decides the mode, each agent's
 * role, and the transition plan (`plan_routing_transition`); this module only
 * turns that answer into words and button states.
 */

export type RoutingModeOption = {
  mode: ChannelRoutingMode;
  label: string;
  description: string;
  /** False while the mode ships in a later change; rendered disabled. */
  available: boolean;
};

export const ROUTING_MODE_OPTIONS: readonly RoutingModeOption[] = [
  {
    mode: "off",
    label: "Off",
    description: "Only @mentioned agents answer.",
    available: true,
  },
  {
    mode: "host",
    label: "Host",
    description:
      "A host agent reads every message and hands it to the right agent.",
    available: true,
  },
  {
    mode: "lead",
    label: "Lead",
    description:
      "One of your agents listens, takes its own work, and hands off the rest.",
    available: false,
  },
  {
    mode: "desktop-router",
    label: "Smart routing",
    description:
      "Buzz picks the agent as you send. No extra messages in the channel.",
    available: false,
  },
];

/** Modes whose routing agent is chosen in the card's picker. */
export function modeNeedsAgent(mode: ChannelRoutingMode): boolean {
  return mode === "host" || mode === "lead";
}

export function routingModeLabel(mode: ChannelRoutingMode): string {
  return (
    ROUTING_MODE_OPTIONS.find((option) => option.mode === mode)?.label ?? mode
  );
}

/** Picker label for a mode that needs an agent ("Host agent"). */
export function routingAgentLabel(mode: ChannelRoutingMode): string {
  return `${routingModeLabel(mode)} agent`;
}

export type RoutingAgentOption = { pubkey: string; name: string };

/**
 * Agents the Host picker offers: every keyed managed agent, local or remote
 * (a host only needs the dispatcher flag, which provider deploys carry too).
 */
export function hostPickerOptions(
  agents: readonly Pick<ManagedAgent, "pubkey" | "name">[],
): RoutingAgentOption[] {
  return agents
    .filter(isDefaultAiEligible)
    .map(({ pubkey, name }) => ({ pubkey, name }))
    .sort((a, b) => a.name.localeCompare(b.name));
}

type NameOf = (pubkey: string) => string;

const ROLE_NOUN: Record<Exclude<RoutingRole, "none">, string> = {
  dispatcher: "host",
  lead: "lead",
};
const ROLE_VERB: Record<Exclude<RoutingRole, "none">, string> = {
  dispatcher: "hosting",
  lead: "leading",
};

/** What one stale agent's restart will do, in plain words. */
/** The other agent still running a role it is losing, if any. */
function holdingAgent(
  agents: readonly AgentRoutingTransition[],
  pubkey: string,
): AgentRoutingTransition | undefined {
  return agents.find(
    (other) =>
      other.pubkey !== pubkey && other.stale && other.runningRole !== "none",
  );
}

function transitionSentence(
  agent: AgentRoutingTransition,
  agents: readonly AgentRoutingTransition[],
  nameOf: NameOf,
): string {
  const name = nameOf(agent.pubkey);
  const { runningRole: from, desiredRole: to } = agent;
  if (from !== "none" && to === "none") {
    return `${name} stops ${ROLE_VERB[from]} after it restarts.`;
  }
  if (from === "none" && to !== "none") {
    // A held gainer (running plain or stopped) waits for the losing agent.
    const blocker = agent.hold ? holdingAgent(agents, agent.pubkey) : undefined;
    return blocker
      ? `${name} becomes the ${ROLE_NOUN[to]} after ${nameOf(blocker.pubkey)} restarts.`
      : `${name} becomes the ${ROLE_NOUN[to]} after it restarts.`;
  }
  if (from !== "none" && to !== "none") {
    return `${name} switches from ${ROLE_NOUN[from]} to ${ROLE_NOUN[to]} after it restarts.`;
  }
  return `${name} restarts to finish switching.`;
}

export type RoutingStatusLine = {
  tone: "on" | "off" | "switching" | "attention";
  text: string;
};

/**
 * The card's single status line: the APPLIED state (what runs now), never
 * just the saved mode.
 */
export function routingStatusLine(
  status: ChannelRoutingStatus,
  nameOf: NameOf,
): RoutingStatusLine {
  const { applied, mode, routingAgent } = status;
  switch (applied.state) {
    case "hosting":
      return { tone: "on", text: `On — ${nameOf(applied.pubkey)} is hosting.` };
    case "leading":
      return { tone: "on", text: `On — ${nameOf(applied.pubkey)} is leading.` };
    case "smart-routing":
      return { tone: "on", text: "On — Buzz picks the agent as you send." };
    case "switching": {
      // Stale agents, plus a held gainer that is not running yet (a launch
      // during the hold would come up plain, so it is part of the switch).
      const sentences = status.agents
        .filter(
          (agent) =>
            agent.stale ||
            (agent.hold && !agent.running && agent.desiredRole !== "none"),
        )
        .map((agent) => transitionSentence(agent, status.agents, nameOf));
      if (mode === "desktop-router") {
        sentences.push("Smart routing turns on right after.");
      }
      return { tone: "switching", text: `Switching — ${sentences.join(" ")}` };
    }
    case "off":
      break;
  }
  if (!modeNeedsAgent(mode)) {
    return { tone: "off", text: "Off — only @mentioned agents answer." };
  }
  const noun = mode === "host" ? "host" : "lead";
  if (routingAgent === null) {
    return {
      tone: "attention",
      text: `Choose a ${noun} agent to turn this on.`,
    };
  }
  const agent = status.agents.find((entry) => entry.pubkey === routingAgent);
  if (agent && !agent.running) {
    return {
      tone: "attention",
      text: `${nameOf(routingAgent)} isn't running. Start it to turn this on.`,
    };
  }
  return { tone: "off", text: "Off — routing is not running." };
}

export type RoutingRestartAffordance = {
  pubkey: string;
  /** Null when no button applies (a remote agent needs a redeploy instead). */
  label: string | null;
  disabled: boolean;
  reason: string | null;
};

/**
 * "Restart now" for each agent whose running role is out of date. Disabled
 * while the plan holds it (another agent still runs a role it is losing) or
 * while it is mid-turn; a remote agent gets a redeploy hint instead.
 */
export function routingRestartAffordances(
  status: ChannelRoutingStatus,
  {
    isWorking,
    nameOf,
  }: { isWorking: (pubkey: string) => boolean; nameOf: NameOf },
): RoutingRestartAffordance[] {
  const stale = status.agents.filter((agent) => agent.stale);
  return stale.map((agent) => {
    const name = nameOf(agent.pubkey);
    if (!agent.local) {
      return {
        pubkey: agent.pubkey,
        label: null,
        disabled: true,
        reason: `Redeploy ${name} to finish switching.`,
      };
    }
    const label = `Restart ${name} now`;
    if (agent.hold) {
      const blocker = holdingAgent(status.agents, agent.pubkey);
      const reason =
        blocker && blocker.runningRole !== "none"
          ? `Waiting for ${nameOf(blocker.pubkey)} to stop ${ROLE_VERB[blocker.runningRole]}.`
          : "Waiting for the previous routing agent to restart.";
      return { pubkey: agent.pubkey, label, disabled: true, reason };
    }
    if (isWorking(agent.pubkey)) {
      return {
        pubkey: agent.pubkey,
        label,
        disabled: true,
        reason: `${name} is in the middle of a turn — restart is available when it finishes.`,
      };
    }
    return { pubkey: agent.pubkey, label, disabled: false, reason: null };
  });
}

/**
 * Whether the transition plan holds this agent's restart. Unknown status
 * (still loading) does not hold: with no saved transition there is nothing
 * to overlap, which is today's behavior.
 */
export function routingHoldFor(
  status: ChannelRoutingStatus | undefined,
  pubkey: string,
): boolean {
  return (
    status?.agents.some((agent) => agent.pubkey === pubkey && agent.hold) ??
    false
  );
}

/** Settings › Agents one-liner: the saved mode and its agent. */
export function routingSummaryText(
  status: ChannelRoutingStatus,
  nameOf: NameOf,
): string {
  const label = routingModeLabel(status.mode);
  if (!modeNeedsAgent(status.mode)) return `Channel routing: ${label}`;
  return status.routingAgent === null
    ? `Channel routing: ${label} (no agent chosen)`
    : `Channel routing: ${label} (${nameOf(status.routingAgent)})`;
}

/** Display name for a pubkey from the managed-agent list. */
export function routingNameOf(
  agents: readonly Pick<ManagedAgent, "pubkey" | "name">[],
): NameOf {
  return (pubkey) =>
    agents.find((agent) => agent.pubkey === pubkey)?.name ?? "Your agent";
}
