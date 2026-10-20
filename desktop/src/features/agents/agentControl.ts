import type { ManagedAgent } from "@/shared/api/types";
import { normalizePubkey } from "@/shared/lib/pubkey";

export const AGENT_CONTROL_REQUEST = "agent_control_request" as const;
export const AGENT_CONTROL_RESULT = "agent_control_result" as const;

export type AgentControlAction = "list" | "start" | "stop" | "restart";

/** An owner's agent asking this Desktop to list, start, stop or restart an agent. */
export type AgentControlRequest =
  | {
      type: typeof AGENT_CONTROL_REQUEST;
      action: "list";
      requestId: string;
    }
  | {
      type: typeof AGENT_CONTROL_REQUEST;
      action: "start" | "stop" | "restart";
      requestId: string;
      /** The target agent's name or hex pubkey. */
      agent: string;
    };

export type AgentControlAgentSummary = {
  name: string;
  pubkey: string;
  status: ManagedAgent["status"];
  lastError?: string;
};

/** The owner→agent reply, sent as an observer control frame. */
export type AgentControlResult = {
  type: typeof AGENT_CONTROL_RESULT;
  requestId: string;
  action: AgentControlAction;
  ok: boolean;
  computer: string;
  error?: string;
  agents?: AgentControlAgentSummary[];
  agent?: AgentControlAgentSummary;
};

/**
 * Requests older than this are not acted on. The observer subscription
 * replays five minutes on connect, so without it an app restart would
 * re-run a start or stop the agent asked for minutes ago.
 */
export const AGENT_CONTROL_MAX_AGE_MS = 60_000;

const ACTIONS: readonly AgentControlAction[] = [
  "list",
  "start",
  "stop",
  "restart",
];

function isText(value: unknown, max: number): value is string {
  return (
    typeof value === "string" &&
    value.trim().length > 0 &&
    value.trim().length <= max
  );
}

/** Parses only the narrow agent-control contract `kuzz agents list|start|stop|restart` sends. */
export function parseAgentControlRequest(
  value: unknown,
): AgentControlRequest | null {
  if (typeof value !== "object" || value === null) return null;
  const payload = value as Record<string, unknown>;
  if (
    payload.type !== AGENT_CONTROL_REQUEST ||
    !isText(payload.requestId, 128) ||
    !ACTIONS.includes(payload.action as AgentControlAction) ||
    typeof payload.request !== "object" ||
    payload.request === null
  ) {
    return null;
  }
  const request = payload.request as Record<string, unknown>;
  const keys = Object.keys(request);
  const action = payload.action as AgentControlAction;
  if (action === "list") {
    if (keys.length > 0) return null;
    return {
      type: AGENT_CONTROL_REQUEST,
      action,
      requestId: payload.requestId,
    };
  }
  if (keys.some((key) => key !== "agent") || !isText(request.agent, 120)) {
    return null;
  }
  return {
    type: AGENT_CONTROL_REQUEST,
    action,
    requestId: payload.requestId,
    agent: request.agent.trim(),
  };
}

/** True while a request sent at `timestamp` (RFC 3339) may still be acted on. */
export function isFreshAgentControlRequest(
  timestamp: string | undefined,
  nowMs: number,
): boolean {
  const sentMs = timestamp ? Date.parse(timestamp) : Number.NaN;
  if (Number.isNaN(sentMs)) return false;
  // A little clock skew into the future is fine; far future is not.
  return nowMs - sentMs <= AGENT_CONTROL_MAX_AGE_MS && sentMs - nowMs <= 60_000;
}

export function summarizeAgent(agent: ManagedAgent): AgentControlAgentSummary {
  return {
    name: agent.name,
    pubkey: agent.pubkey,
    status: agent.status,
    // Bounded so a long list stays inside one encrypted observer frame.
    ...(agent.lastError ? { lastError: agent.lastError.slice(0, 300) } : {}),
  };
}

export type AgentControlPlan =
  /** The target agent is not on this computer: stay silent, its host answers. */
  | { kind: "ignore" }
  | { kind: "reply"; ok: boolean; error?: string; agents?: ManagedAgent[] }
  | {
      kind: "run";
      action: "start" | "stop" | "restart";
      agent: ManagedAgent;
    };

/**
 * Decides what this computer does with a request. `ownedAgents` must be this
 * computer's agents of the account the request was sent to.
 */
export function planAgentControl(
  request: AgentControlRequest,
  ownedAgents: readonly ManagedAgent[],
): AgentControlPlan {
  if (request.action === "list") {
    return {
      kind: "reply",
      ok: true,
      agents: [...ownedAgents].sort((left, right) =>
        left.name.localeCompare(right.name, undefined, { sensitivity: "base" }),
      ),
    };
  }
  const wanted = request.agent.trim();
  const byPubkey = /^[0-9a-f]{64}$/i.test(wanted)
    ? ownedAgents.filter(
        (agent) => normalizePubkey(agent.pubkey) === normalizePubkey(wanted),
      )
    : [];
  const matches =
    byPubkey.length > 0
      ? byPubkey
      : ownedAgents.filter(
          (agent) =>
            agent.name.trim().toLocaleLowerCase() ===
            wanted.toLocaleLowerCase(),
        );
  if (matches.length === 0) return { kind: "ignore" };
  if (matches.length > 1) {
    return {
      kind: "reply",
      ok: false,
      error: `More than one agent here is named "${wanted}". Use its pubkey (kuzz agents list).`,
    };
  }
  return { kind: "run", action: request.action, agent: matches[0] };
}

export function buildAgentControlResult(
  request: AgentControlRequest,
  computer: string,
  outcome: {
    ok: boolean;
    error?: string;
    agents?: readonly ManagedAgent[];
    agent?: ManagedAgent;
  },
): AgentControlResult {
  return {
    type: AGENT_CONTROL_RESULT,
    requestId: request.requestId,
    action: request.action,
    ok: outcome.ok,
    computer,
    ...(outcome.error ? { error: outcome.error } : {}),
    ...(outcome.agents ? { agents: outcome.agents.map(summarizeAgent) } : {}),
    ...(outcome.agent ? { agent: summarizeAgent(outcome.agent) } : {}),
  };
}
