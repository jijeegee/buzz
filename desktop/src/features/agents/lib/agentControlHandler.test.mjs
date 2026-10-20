import assert from "node:assert/strict";
import { beforeEach, test } from "node:test";

import { AGENT_CONTROL_REQUEST } from "../agentControl.ts";
import {
  _resetAgentControlHandler,
  handleAgentControlRequest,
} from "./agentControlHandler.ts";

const OWNER = "0".repeat(64);
const OTHER_OWNER = "9".repeat(64);
const REQUESTER = "1".repeat(64);
const SCOUT = "2".repeat(64);
const NOW = Date.parse("2026-10-20T12:00:00Z");
const SENT_AT = "2026-10-20T11:59:55Z";
const SCOPE = { ownerPubkey: OWNER, relayUrl: "wss://relay.example" };

function agent(overrides = {}) {
  return {
    pubkey: SCOUT,
    name: "Scout",
    ownerPubkey: OWNER,
    status: "stopped",
    lastError: null,
    backend: { type: "local" },
    ...overrides,
  };
}

function request(action, target = "Scout", requestId = `req-${action}`) {
  return action === "list"
    ? { type: AGENT_CONTROL_REQUEST, action, requestId }
    : { type: AGENT_CONTROL_REQUEST, action, requestId, agent: target };
}

function harness(agents) {
  const calls = [];
  const replies = [];
  const deps = {
    listAgents: async () => agents,
    start: async (pubkey, options) => {
      calls.push(["start", pubkey, options]);
      return {
        ...agents.find((item) => item.pubkey === pubkey),
        status: "running",
      };
    },
    stop: async (pubkey) => {
      calls.push(["stop", pubkey]);
      return {
        ...agents.find((item) => item.pubkey === pubkey),
        status: "stopped",
      };
    },
    computerName: async () => "DESKTOP-1",
    reply: async (to, result) => {
      calls.push(["reply", to]);
      replies.push(result);
    },
    notify: () => {},
    now: () => NOW,
  };
  return { calls, replies, deps };
}

beforeEach(() => _resetAgentControlHandler());

test("starts a stopped local agent scoped to the open account and replies", async () => {
  const { calls, replies, deps } = harness([agent()]);
  await handleAgentControlRequest(
    REQUESTER,
    request("start"),
    SENT_AT,
    SCOPE,
    deps,
  );
  assert.deepEqual(calls[0], [
    "start",
    SCOUT,
    { expectedRelayUrl: SCOPE.relayUrl, expectedSignerPubkey: OWNER },
  ]);
  assert.deepEqual(calls[1], ["reply", REQUESTER]);
  assert.equal(replies[0].ok, true);
  assert.equal(replies[0].computer, "DESKTOP-1");
  assert.equal(replies[0].agent.status, "running");
});

test("stays silent when the agent is not on this computer or not this account's", async () => {
  const { calls, deps } = harness([agent({ ownerPubkey: OTHER_OWNER })]);
  await handleAgentControlRequest(
    REQUESTER,
    request("start"),
    SENT_AT,
    SCOPE,
    deps,
  );
  assert.deepEqual(calls, []);
});

test("list answers with this account's agents only", async () => {
  const { replies, deps } = harness([
    agent(),
    agent({ name: "Theirs", pubkey: "3".repeat(64), ownerPubkey: OTHER_OWNER }),
  ]);
  await handleAgentControlRequest(
    REQUESTER,
    request("list"),
    SENT_AT,
    SCOPE,
    deps,
  );
  assert.deepEqual(
    replies[0].agents.map((item) => item.name),
    ["Scout"],
  );
});

test("a replayed or stale request is not run again", async () => {
  const { calls, deps } = harness([agent()]);
  await handleAgentControlRequest(
    REQUESTER,
    request("start"),
    SENT_AT,
    SCOPE,
    deps,
  );
  await handleAgentControlRequest(
    REQUESTER,
    request("start"),
    SENT_AT,
    SCOPE,
    deps,
  );
  await handleAgentControlRequest(
    REQUESTER,
    request("start", "Scout", "old"),
    "2026-10-20T11:50:00Z",
    SCOPE,
    deps,
  );
  assert.equal(calls.filter(([kind]) => kind === "start").length, 1);
});

test("starting a running agent or stopping a stopped one is a no-op success", async () => {
  const running = harness([agent({ status: "running" })]);
  await handleAgentControlRequest(
    REQUESTER,
    request("start"),
    SENT_AT,
    SCOPE,
    running.deps,
  );
  assert.deepEqual(running.calls, [["reply", REQUESTER]]);
  assert.equal(running.replies[0].ok, true);

  const stopped = harness([agent()]);
  await handleAgentControlRequest(
    REQUESTER,
    request("stop"),
    SENT_AT,
    SCOPE,
    stopped.deps,
  );
  assert.deepEqual(stopped.calls, [["reply", REQUESTER]]);
});

test("restart stops then starts a running agent", async () => {
  const { calls, replies, deps } = harness([agent({ status: "running" })]);
  await handleAgentControlRequest(
    REQUESTER,
    request("restart"),
    SENT_AT,
    SCOPE,
    deps,
  );
  assert.deepEqual(
    calls.map(([kind]) => kind),
    ["stop", "start", "reply"],
  );
  assert.equal(replies[0].ok, true);
});

test("an agent restarting itself hears back before it is stopped", async () => {
  const self = agent({ pubkey: REQUESTER, status: "running" });
  const { calls, replies, deps } = harness([self]);
  await handleAgentControlRequest(
    REQUESTER,
    request("restart", REQUESTER),
    SENT_AT,
    SCOPE,
    deps,
  );
  assert.deepEqual(
    calls.map(([kind]) => kind),
    ["reply", "stop", "start"],
  );
  assert.equal(replies.length, 1);
});

test("a failed start replies with the error", async () => {
  const { replies, deps } = harness([agent()]);
  deps.start = async () => {
    throw "Runtime is not installed.";
  };
  await handleAgentControlRequest(
    REQUESTER,
    request("start"),
    SENT_AT,
    SCOPE,
    deps,
  );
  assert.equal(replies[0].ok, false);
  assert.equal(replies[0].error, "Runtime is not installed.");
});

const PROVIDER = { type: "provider", id: "cloud", config: {} };

test("a provider-hosted agent is not stopped here; restart redeploys without a stop", async () => {
  const stop = harness([agent({ status: "deployed", backend: PROVIDER })]);
  await handleAgentControlRequest(
    REQUESTER,
    request("stop"),
    SENT_AT,
    SCOPE,
    stop.deps,
  );
  assert.deepEqual(
    stop.calls.map(([kind]) => kind),
    ["reply"],
  );
  assert.equal(stop.replies[0].ok, false);
  assert.match(stop.replies[0].error, /hosting provider/);

  const restart = harness([agent({ status: "deployed", backend: PROVIDER })]);
  await handleAgentControlRequest(
    REQUESTER,
    request("restart"),
    SENT_AT,
    SCOPE,
    restart.deps,
  );
  assert.deepEqual(
    restart.calls.map(([kind]) => kind),
    ["start", "reply"],
  );
});

test("two quick restarts of one agent run one after the other", async () => {
  const agents = [agent({ status: "running" })];
  const { calls, deps } = harness(agents);
  const steps = [];
  deps.stop = async (pubkey) => {
    steps.push("stop-begin");
    await new Promise((resolve) => setTimeout(resolve, 10));
    steps.push("stop-end");
    agents[0] = { ...agents[0], status: "stopped" };
    calls.push(["stop", pubkey]);
    return agents[0];
  };
  deps.start = async (pubkey) => {
    steps.push("start");
    agents[0] = { ...agents[0], status: "running" };
    calls.push(["start", pubkey]);
    return agents[0];
  };
  await Promise.all([
    handleAgentControlRequest(
      REQUESTER,
      request("restart", "Scout", "r1"),
      SENT_AT,
      SCOPE,
      deps,
    ),
    handleAgentControlRequest(
      REQUESTER,
      request("restart", "Scout", "r2"),
      SENT_AT,
      SCOPE,
      deps,
    ),
  ]);
  assert.deepEqual(steps, [
    "stop-begin",
    "stop-end",
    "start",
    "stop-begin",
    "stop-end",
    "start",
  ]);
});
