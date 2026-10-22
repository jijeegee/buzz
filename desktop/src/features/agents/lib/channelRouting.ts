import { isDefaultAiEligible } from "@/features/agents/lib/defaultAi";
import type { ManagedAgent } from "@/shared/api/types";
import type {
  AgentRoutingTransition,
  ChannelRoutingMode,
  ChannelRoutingStatus,
} from "@/shared/api/tauriChannelRouting";
import { type MessageKey, t } from "@/shared/i18n";

/**
 * Pure projections of the Rust-owned channel routing status for the Agents
 * page card and the Settings summary. Rust decides the mode, each agent's
 * role, and the transition plan (`plan_routing_transition`); this module only
 * turns that answer into words and button states.
 */

export type RoutingModeOption = {
  mode: ChannelRoutingMode;
  label: MessageKey;
  description: MessageKey;
  /** False while the mode ships in a later change; rendered disabled. */
  available: boolean;
};

export const ROUTING_MODE_OPTIONS: readonly RoutingModeOption[] = [
  {
    mode: "off",
    label: "agents.routing.mode.off",
    description: "agents.routing.mode.offDescription",
    available: true,
  },
  {
    mode: "host",
    label: "agents.routing.mode.host",
    description: "agents.routing.mode.hostDescription",
    available: true,
  },
  {
    mode: "lead",
    label: "agents.routing.mode.lead",
    description: "agents.routing.mode.leadDescription",
    available: true,
  },
  {
    mode: "desktop-router",
    label: "agents.routing.mode.smart",
    description: "agents.routing.mode.smartDescription",
    available: true,
  },
];

/** Modes whose routing agent is chosen in the card's picker. */
export function modeNeedsAgent(mode: ChannelRoutingMode): boolean {
  return mode === "host" || mode === "lead";
}

export function routingModeLabel(mode: ChannelRoutingMode): string {
  const option = ROUTING_MODE_OPTIONS.find((entry) => entry.mode === mode);
  return option ? t(option.label) : mode;
}

/** Picker label for a mode that needs an agent ("Host agent"). */
export function routingAgentLabel(mode: ChannelRoutingMode): string {
  return t("agents.routing.agentPickerLabel", { mode });
}

export type RoutingAgentOption = {
  pubkey: string;
  name: string;
  /** Why this agent can't take the mode's role; rendered disabled. */
  disabledReason: string | null;
};

/**
 * Mirrors Rust's refusal (`lead_unavailable_reason`); Rust still enforces.
 * English source of `agents.routing.leadNeedsLocal`; the picker shows `t()`.
 */
export const LEAD_NEEDS_LOCAL_REASON =
  "runs remotely — Lead needs an agent on this computer";

/**
 * Agents the picker offers for `mode`: every keyed managed agent, sorted by
 * name. A host only needs the dispatcher flag, which provider deploys carry
 * too, so any backend can host. A lead reads a rules file written on this
 * computer, so remote agents are listed but disabled with the reason.
 */
export function routingPickerOptions(
  mode: ChannelRoutingMode,
  agents: readonly Pick<ManagedAgent, "pubkey" | "name" | "backend">[],
): RoutingAgentOption[] {
  return agents
    .filter(isDefaultAiEligible)
    .map(({ pubkey, name, backend }) => ({
      pubkey,
      name,
      disabledReason:
        mode === "lead" && backend.type !== "local"
          ? t("agents.routing.leadNeedsLocal")
          : null,
    }))
    .sort((a, b) => a.name.localeCompare(b.name));
}

type NameOf = (pubkey: string) => string;

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
    return t("agents.routing.transition.stops", { name, role: from });
  }
  if (from === "none" && to !== "none") {
    // A held gainer (running plain or stopped) waits for the losing agent.
    const blocker = agent.hold ? holdingAgent(agents, agent.pubkey) : undefined;
    return blocker
      ? t("agents.routing.transition.becomesAfterOther", {
          name,
          role: to,
          other: nameOf(blocker.pubkey),
        })
      : t("agents.routing.transition.becomes", { name, role: to });
  }
  if (from !== "none" && to !== "none") {
    return t("agents.routing.transition.switches", { name, from, to });
  }
  return t("agents.routing.transition.restarts", { name });
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
      return {
        tone: "on",
        text: t("agents.routing.status.hosting", {
          name: nameOf(applied.pubkey),
        }),
      };
    case "leading":
      return {
        tone: "on",
        text: t("agents.routing.status.leading", {
          name: nameOf(applied.pubkey),
        }),
      };
    case "smart-routing":
      return { tone: "on", text: t("agents.routing.status.smart") };
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
        sentences.push(t("agents.routing.status.smartAfter"));
      }
      return {
        tone: "switching",
        text: t("agents.routing.status.switching", {
          steps: sentences.join(" "),
        }),
      };
    }
    case "off":
      break;
  }
  if (!modeNeedsAgent(mode)) {
    return { tone: "off", text: t("agents.routing.status.off") };
  }
  if (routingAgent === null) {
    return {
      tone: "attention",
      text: t("agents.routing.status.chooseAgent", { mode }),
    };
  }
  const agent = status.agents.find((entry) => entry.pubkey === routingAgent);
  if (agent && !agent.running) {
    return {
      tone: "attention",
      text: t("agents.routing.status.notRunning", {
        name: nameOf(routingAgent),
      }),
    };
  }
  return { tone: "off", text: t("agents.routing.status.offNotRunning") };
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
        reason: t("agents.routing.restart.redeploy", { name }),
      };
    }
    const label = t("agents.routing.restart.button", { name });
    if (agent.hold) {
      const blocker = holdingAgent(status.agents, agent.pubkey);
      const reason =
        blocker && blocker.runningRole !== "none"
          ? t("agents.routing.restart.waitingFor", {
              name: nameOf(blocker.pubkey),
              role: blocker.runningRole,
            })
          : t("agents.routing.restart.waitingPrevious");
      return { pubkey: agent.pubkey, label, disabled: true, reason };
    }
    if (isWorking(agent.pubkey)) {
      return {
        pubkey: agent.pubkey,
        label,
        disabled: true,
        reason: t("agents.routing.restart.busy", { name }),
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

/**
 * Whether this agent's restart applies a routing switch right now: a local
 * agent the plan marks stale (running role ≠ saved role) and not held.
 * Auto-restart fires these as soon as the agent is idle instead of waiting
 * out the config-drift quiescence window.
 */
export function routingTransitionFor(
  status: ChannelRoutingStatus | undefined,
  pubkey: string,
): boolean {
  return (
    status?.agents.some(
      (agent) =>
        agent.pubkey === pubkey && agent.local && agent.stale && !agent.hold,
    ) ?? false
  );
}

/** Settings › Agents one-liner: the saved mode and its agent. */
export function routingSummaryText(
  status: ChannelRoutingStatus,
  nameOf: NameOf,
): string {
  const label = routingModeLabel(status.mode);
  if (!modeNeedsAgent(status.mode))
    return t("agents.routing.summary", { mode: label });
  return status.routingAgent === null
    ? t("agents.routing.summaryNoAgent", { mode: label })
    : t("agents.routing.summaryAgent", {
        mode: label,
        name: nameOf(status.routingAgent),
      });
}

/** Display name for a pubkey from the managed-agent list. */
export function routingNameOf(
  agents: readonly Pick<ManagedAgent, "pubkey" | "name">[],
): NameOf {
  return (pubkey) =>
    agents.find((agent) => agent.pubkey === pubkey)?.name ??
    t("agents.routing.yourAgent");
}
