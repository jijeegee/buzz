import assert from "node:assert/strict";
import test from "node:test";

import { shouldShowRuntimeTab } from "./profileRuntimeGates.ts";

// The Runtime tab's reachability gate. Cases: local-backend and
// remote-backend managed agents, a key-less persona definition, and non-owner
// viewers.

const KEYED = "ab".repeat(32);

function agent(overrides = {}) {
  return {
    pubkey: KEYED,
    name: "Scout",
    backend: { type: "local" },
    isDefaultAi: false,
    ...overrides,
  };
}

const bareTab = {
  canOpenAgentLogs: false,
  diagnosticsFieldCount: 0,
  instanceCount: 0,
  isBot: true,
  isOwner: true,
  runtimeFieldCount: 0,
};

test("an owner viewing a local- or remote-backend managed agent gets the tab", () => {
  assert.equal(
    shouldShowRuntimeTab({ ...bareTab, managedAgent: agent() }),
    true,
  );
  const remote = agent({ backend: { type: "provider", id: "cloud-runner" } });
  assert.equal(
    shouldShowRuntimeTab({ ...bareTab, managedAgent: remote }),
    true,
  );
});

test("a key-less persona definition still shows the tab", () => {
  for (const pubkey of ["", "   "]) {
    const managedAgent = agent({ pubkey });
    assert.equal(shouldShowRuntimeTab({ ...bareTab, managedAgent }), true);
  }
});

test("a non-owner never gets the tab, whatever the agent", () => {
  const managedAgent = agent();
  for (const isOwner of [false, undefined]) {
    assert.equal(
      shouldShowRuntimeTab({ ...bareTab, isOwner, managedAgent }),
      false,
    );
    assert.equal(
      shouldShowRuntimeTab({
        ...bareTab,
        canOpenAgentLogs: true,
        diagnosticsFieldCount: 2,
        instanceCount: 3,
        isOwner,
        managedAgent,
        runtimeFieldCount: 4,
      }),
      false,
    );
  }
});

test("the tab needs an agent profile and some runtime material", () => {
  // A human profile never gets a Runtime tab, even for its owner.
  assert.equal(
    shouldShowRuntimeTab({ ...bareTab, isBot: false, managedAgent: agent() }),
    false,
  );
  // An agent with nothing to show (not managed here, no fields, no
  // instances, no diagnostics, no logs) gets no tab.
  assert.equal(
    shouldShowRuntimeTab({ ...bareTab, managedAgent: undefined }),
    false,
  );
  // Any one source of runtime material is enough.
  for (const extra of [
    { runtimeFieldCount: 1 },
    { instanceCount: 1 },
    { diagnosticsFieldCount: 1 },
    { canOpenAgentLogs: true },
  ]) {
    assert.equal(
      shouldShowRuntimeTab({ ...bareTab, ...extra, managedAgent: undefined }),
      true,
      JSON.stringify(extra),
    );
  }
});
