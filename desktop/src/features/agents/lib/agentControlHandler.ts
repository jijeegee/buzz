import type { ManagedAgent } from "@/shared/api/types";
import { normalizePubkey } from "@/shared/lib/pubkey";
import {
  buildAgentControlResult,
  isFreshAgentControlRequest,
  planAgentControl,
  type AgentControlRequest,
  type AgentControlResult,
} from "../agentControl";

export type AgentControlScope = {
  /** The open account; requests reach only its observer subscription. */
  ownerPubkey: string;
  relayUrl: string;
};

export type AgentControlDeps = {
  listAgents: () => Promise<ManagedAgent[]>;
  start: (
    pubkey: string,
    options: { expectedRelayUrl: string; expectedSignerPubkey: string },
  ) => Promise<ManagedAgent>;
  stop: (pubkey: string) => Promise<ManagedAgent>;
  computerName: () => Promise<string>;
  reply: (agentPubkey: string, result: AgentControlResult) => Promise<void>;
  /** Tells the owner on screen what an agent just did. */
  notify: (message: string) => void;
  now: () => number;
};

/** Request ids already handled, so a replayed frame never runs twice. */
const handledRequestIds = new Set<string>();
const MAX_HANDLED_REQUEST_IDS = 500;

function claimRequest(requestId: string): boolean {
  if (handledRequestIds.has(requestId)) return false;
  handledRequestIds.add(requestId);
  if (handledRequestIds.size > MAX_HANDLED_REQUEST_IDS) {
    const oldest = handledRequestIds.values().next().value;
    if (oldest !== undefined) handledRequestIds.delete(oldest);
  }
  return true;
}

/**
 * One change at a time per agent: two quick restarts must not run their
 * stop and start steps interleaved.
 */
const agentQueues = new Map<string, Promise<void>>();

function runInAgentQueue(pubkey: string, task: () => Promise<void>) {
  const key = normalizePubkey(pubkey);
  const run = (agentQueues.get(key) ?? Promise.resolve()).then(task);
  const settled = run.catch(() => {});
  agentQueues.set(key, settled);
  void settled.then(() => {
    if (agentQueues.get(key) === settled) agentQueues.delete(key);
  });
  return run;
}

/** Test-only: forget handled request ids and queues. */
export function _resetAgentControlHandler() {
  handledRequestIds.clear();
  agentQueues.clear();
}

function isRunning(agent: ManagedAgent) {
  return agent.status === "running" || agent.status === "deployed";
}

function errorText(error: unknown) {
  if (error instanceof Error) return error.message;
  return typeof error === "string" ? error : "Something went wrong.";
}

const PAST_TENSE = {
  start: "started",
  stop: "stopped",
  restart: "restarted",
} as const;

/**
 * Runs one owner's-agent request to list, start, stop or restart an agent on
 * this computer and replies to the requesting agent. A request for an agent
 * that is not on this computer is left to the computer that has it.
 */
export async function handleAgentControlRequest(
  requesterPubkey: string,
  request: AgentControlRequest,
  sentAt: string,
  scope: AgentControlScope,
  deps: AgentControlDeps,
): Promise<void> {
  if (!isFreshAgentControlRequest(sentAt, deps.now())) return;
  if (!claimRequest(request.requestId)) return;

  const owner = normalizePubkey(scope.ownerPubkey);
  // Same account rule as the agents page: records with no recorded owner
  // belong to the open account.
  const agents = (await deps.listAgents()).filter(
    (agent) =>
      !agent.ownerPubkey || normalizePubkey(agent.ownerPubkey) === owner,
  );
  const plan = planAgentControl(request, agents);
  if (plan.kind === "ignore") return;

  const computer = await deps.computerName();
  const send = (outcome: Parameters<typeof buildAgentControlResult>[2]) =>
    deps.reply(
      requesterPubkey,
      buildAgentControlResult(request, computer, outcome),
    );

  if (plan.kind === "reply") {
    await send(plan);
    return;
  }

  const { action } = plan;
  await runInAgentQueue(plan.agent.pubkey, async () => {
    // Re-read: an earlier queued request may have changed its state.
    const agent =
      (await deps.listAgents()).find(
        (candidate) =>
          normalizePubkey(candidate.pubkey) ===
          normalizePubkey(plan.agent.pubkey),
      ) ?? plan.agent;
    await runAgentControl(
      agent,
      action,
      requesterPubkey,
      agents,
      scope,
      send,
      deps,
    );
  });
}

async function runAgentControl(
  agent: ManagedAgent,
  action: "start" | "stop" | "restart",
  requesterPubkey: string,
  agents: readonly ManagedAgent[],
  scope: AgentControlScope,
  send: (
    outcome: Parameters<typeof buildAgentControlResult>[2],
  ) => Promise<void>,
  deps: AgentControlDeps,
) {
  const owner = normalizePubkey(scope.ownerPubkey);
  const isLocal = agent.backend.type === "local";
  const isSelf =
    normalizePubkey(agent.pubkey) === normalizePubkey(requesterPubkey);
  const requesterName =
    agents.find(
      (candidate) =>
        normalizePubkey(candidate.pubkey) === normalizePubkey(requesterPubkey),
    )?.name ?? "An agent";
  const startOptions = {
    expectedRelayUrl: scope.relayUrl,
    expectedSignerPubkey: owner,
  };

  if (action === "start" && isRunning(agent)) {
    await send({ ok: true, agent });
    return;
  }
  if (action === "stop" && !isRunning(agent)) {
    await send({ ok: true, agent });
    return;
  }
  // The app stops a provider-hosted agent by a chat command, not here.
  if (action === "stop" && !isLocal) {
    await send({
      ok: false,
      error: `${agent.name} runs on a hosting provider, so it can't be stopped this way. Ask the owner to stop it in the Kuzz app.`,
      agent,
    });
    return;
  }

  // An agent stopping or restarting itself would lose the reply with its
  // process, so it hears back before the change.
  if (isSelf && action !== "start") {
    await send({ ok: true, agent });
  }
  try {
    let result: ManagedAgent;
    if (action === "start") {
      result = await deps.start(agent.pubkey, startOptions);
    } else if (action === "stop") {
      result = await deps.stop(agent.pubkey);
    } else {
      // Same as the app's restart: only a local agent is stopped first; a
      // provider agent is redeployed by the start.
      if (isLocal && isRunning(agent)) await deps.stop(agent.pubkey);
      result = await deps.start(agent.pubkey, startOptions);
    }
    deps.notify(
      isSelf
        ? `${agent.name} ${PAST_TENSE[action]} itself.`
        : `${requesterName} ${PAST_TENSE[action]} ${agent.name}.`,
    );
    if (!isSelf) await send({ ok: true, agent: result });
  } catch (error) {
    if (!isSelf) await send({ ok: false, error: errorText(error), agent });
    else console.error("Agent self control failed", agent.pubkey, error);
  }
}
