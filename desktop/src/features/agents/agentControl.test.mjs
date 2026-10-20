import assert from "node:assert/strict";
import test from "node:test";

import {
  AGENT_CONTROL_REQUEST,
  AGENT_CONTROL_RESULT,
  buildAgentControlResult,
  isFreshAgentControlRequest,
  parseAgentControlRequest,
  planAgentControl,
} from "./agentControl.ts";

const SCOUT = "a".repeat(64);
const HELPER = "b".repeat(64);

function agent(overrides = {}) {
  return {
    pubkey: SCOUT,
    name: "Scout",
    ownerPubkey: "c".repeat(64),
    status: "stopped",
    lastError: null,
    ...overrides,
  };
}

function payload(overrides = {}) {
  return {
    type: AGENT_CONTROL_REQUEST,
    action: "start",
    requestId: "request-1",
    request: { agent: "Scout" },
    ...overrides,
  };
}

test("parses start/stop/restart with a target and list without one", () => {
  for (const action of ["start", "stop", "restart"]) {
    assert.deepEqual(parseAgentControlRequest(payload({ action })), {
      type: AGENT_CONTROL_REQUEST,
      action,
      requestId: "request-1",
      agent: "Scout",
    });
  }
  assert.deepEqual(
    parseAgentControlRequest(payload({ action: "list", request: {} })),
    { type: AGENT_CONTROL_REQUEST, action: "list", requestId: "request-1" },
  );
});

test("rejects anything outside the narrow contract", () => {
  const rejected = [
    null,
    payload({ type: "agent_management_request" }),
    payload({ action: "delete" }),
    payload({ requestId: " " }),
    payload({ request: null }),
    payload({ request: {} }),
    payload({ request: { agent: "Scout", force: true } }),
    payload({ request: { agent: "x".repeat(121) } }),
    payload({ action: "list", request: { agent: "Scout" } }),
  ];
  for (const value of rejected) {
    assert.equal(parseAgentControlRequest(value), null, JSON.stringify(value));
  }
});

test("acts only on recent requests", () => {
  const now = Date.parse("2026-10-20T12:00:00Z");
  assert.equal(isFreshAgentControlRequest("2026-10-20T11:59:30Z", now), true);
  assert.equal(isFreshAgentControlRequest("2026-10-20T11:58:00Z", now), false);
  assert.equal(isFreshAgentControlRequest("2026-10-20T12:05:00Z", now), false);
  assert.equal(isFreshAgentControlRequest("not a time", now), false);
  assert.equal(isFreshAgentControlRequest(undefined, now), false);
});

test("list answers with this computer's agents sorted by name", () => {
  const plan = planAgentControl(
    parseAgentControlRequest(payload({ action: "list", request: {} })),
    [agent({ name: "zed", pubkey: HELPER }), agent({ name: "Alpha" })],
  );
  assert.equal(plan.kind, "reply");
  assert.deepEqual(
    plan.agents.map((item) => item.name),
    ["Alpha", "zed"],
  );
});

test("finds the target by name (any case) or pubkey; stays silent when absent", () => {
  const agents = [agent(), agent({ name: "Helper", pubkey: HELPER })];
  const byName = planAgentControl(
    parseAgentControlRequest(payload({ request: { agent: "scout" } })),
    agents,
  );
  assert.equal(byName.kind, "run");
  assert.equal(byName.agent.pubkey, SCOUT);

  const byPubkey = planAgentControl(
    parseAgentControlRequest(
      payload({ action: "stop", request: { agent: HELPER.toUpperCase() } }),
    ),
    agents,
  );
  assert.equal(byPubkey.kind, "run");
  assert.equal(byPubkey.action, "stop");
  assert.equal(byPubkey.agent.pubkey, HELPER);

  assert.deepEqual(
    planAgentControl(
      parseAgentControlRequest(payload({ request: { agent: "Nobody" } })),
      agents,
    ),
    { kind: "ignore" },
  );
});

test("two agents with the same name are refused, not guessed", () => {
  const plan = planAgentControl(parseAgentControlRequest(payload()), [
    agent(),
    agent({ pubkey: HELPER }),
  ]);
  assert.equal(plan.kind, "reply");
  assert.equal(plan.ok, false);
  assert.match(plan.error, /pubkey/);
});

test("result carries the request id, computer and a bounded summary", () => {
  const request = parseAgentControlRequest(payload());
  const result = buildAgentControlResult(request, "DESKTOP-1", {
    ok: true,
    agent: agent({ status: "running", lastError: "e".repeat(1_000) }),
  });
  assert.equal(result.type, AGENT_CONTROL_RESULT);
  assert.equal(result.requestId, "request-1");
  assert.equal(result.action, "start");
  assert.equal(result.computer, "DESKTOP-1");
  assert.equal(result.agent.status, "running");
  assert.equal(result.agent.lastError.length, 300);
  assert.equal("error" in result, false);
});
