import assert from "node:assert/strict";
import test from "node:test";

import {
  defaultAiToggleFor,
  shouldShowRuntimeTab,
} from "./profileRuntimeGates.ts";

// The Runtime tab is the only surface carrying the Default AI row, so these
// two gates together are the reachability condition the first version of the
// switch failed (it sat in a slot that never mounted for agents). Cases:
// local-backend and remote-backend managed agents, a key-less persona
// definition, and non-owner viewers.

const KEYED = "ab".repeat(32);
const handler = () => {};

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

test("an owner viewing a local-backend managed agent gets the tab and the toggle", () => {
  const managedAgent = agent();
  assert.equal(shouldShowRuntimeTab({ ...bareTab, managedAgent }), true);
  assert.equal(
    defaultAiToggleFor({ handler, isOwner: true, managedAgent }),
    handler,
  );
});

test("a remote-backend managed agent is starrable too (the star is backend-agnostic)", () => {
  const managedAgent = agent({
    backend: { type: "provider", id: "cloud-runner" },
  });
  assert.equal(shouldShowRuntimeTab({ ...bareTab, managedAgent }), true);
  assert.equal(
    defaultAiToggleFor({ handler, isOwner: true, managedAgent }),
    handler,
  );
});

test("a key-less persona definition shows the tab but never a Default AI row", () => {
  for (const pubkey of ["", "   "]) {
    const managedAgent = agent({ pubkey });
    assert.equal(shouldShowRuntimeTab({ ...bareTab, managedAgent }), true);
    assert.equal(
      defaultAiToggleFor({ handler, isOwner: true, managedAgent }),
      undefined,
    );
  }
});

test("a non-owner never gets the tab or the toggle, whatever the agent", () => {
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
    assert.equal(
      defaultAiToggleFor({ handler, isOwner, managedAgent }),
      undefined,
    );
  }
});

test("without a managed agent the toggle is absent even for an owner", () => {
  assert.equal(
    defaultAiToggleFor({ handler, isOwner: true, managedAgent: undefined }),
    undefined,
  );
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
