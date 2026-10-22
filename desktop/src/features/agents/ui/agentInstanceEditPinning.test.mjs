import assert from "node:assert/strict";
import test from "node:test";

import { requiredCredentialEnvKeys } from "./agentConfigOptions.tsx";
import {
  computeEditAgentFormValidity,
  hasMissingRequiredEnvKey,
  resolveAgentCommandUpdate,
  resolveEffortSubmission,
} from "./personaRuntimeModel.ts";

// ── Edit dialog submission: the agent owns every setting ─────────────────────
//
// An agent made from a preset owns a copy of the preset's settings. The edit
// dialog validates and saves the agent's own values; there is no "inherit from
// the preset" state, so nothing here reads the preset.

const agent = {
  name: "test-agent",
  agentCommand: "claude-cmd",
  acpCommand: "acp",
};

test("gate_validatesTheAgentsOwnProviderAndEnv", () => {
  const requiredKeys = requiredCredentialEnvKeys("kuzz-agent", "anthropic");
  assert.equal(hasMissingRequiredEnvKey(requiredKeys, {}), true);
  assert.equal(
    hasMissingRequiredEnvKey(requiredKeys, { ANTHROPIC_API_KEY: "sk-agent" }),
    false,
    "the agent's own env satisfies the gate",
  );
});

test("agentCommandUpdate_sendsOnlyARealChange", () => {
  assert.equal(
    resolveAgentCommandUpdate({
      agentCommand: "claude-cmd",
      originalAgentCommand: "claude-cmd",
    }),
    undefined,
    "an unchanged command is not sent",
  );
  assert.equal(
    resolveAgentCommandUpdate({
      agentCommand: " kuzz-agent-cmd ",
      originalAgentCommand: "claude-cmd",
    }),
    "kuzz-agent-cmd",
  );
});

test("editValidity_allowlistWithEmptyList_blocksSave", () => {
  // PR #1667 review: the edit-path crash-loop guard (respondToValid) was
  // never exercised — every other row uses a non-allowlist mode. An agent
  // saved as allowlist-with-empty-list crash-loops at startup.
  const base = {
    name: agent.name,
    parallelism: "1",
    agentAcpCommand: agent.acpCommand,
    acpCommand: agent.acpCommand,
    selectedRuntimeId: "claude",
    agentCommand: agent.agentCommand,
    requiredEnvKeyMissing: false,
  };
  assert.equal(
    computeEditAgentFormValidity({
      ...base,
      respondTo: "allowlist",
      respondToAllowlistLength: 0,
    }),
    false,
    "allowlist with an empty list must block Save",
  );
  assert.equal(
    computeEditAgentFormValidity({
      ...base,
      respondTo: "allowlist",
      respondToAllowlistLength: 1,
    }),
    true,
    "allowlist with at least one pubkey must allow Save",
  );
});

// ── Effort write is Save-gated ──────────────────────────────────────────────

test("effortWrite_persistsRealChange", () => {
  // A plain effort edit: the setter persists the new value.
  const submission = resolveEffortSubmission({
    effortLevel: "high",
    originalEffortLevel: null,
  });
  assert.deepEqual(submission, { persist: true, level: "high" });
});

test("effortWrite_noOp_whenSelectionUnchanged", () => {
  // Re-selecting the currently-effective value (or a name-only Save) writes
  // nothing, so an unrelated edit never rewrites the effort column.
  const submission = resolveEffortSubmission({
    effortLevel: "high",
    originalEffortLevel: "high",
  });
  assert.equal(submission.persist, false);
});

test("effortWrite_clearToAdapterDefault_persistsNull", () => {
  // Clearing an existing pin to the adapter-default sentinel is a real change
  // and must persist null (revert), not be treated as a no-op.
  const submission = resolveEffortSubmission({
    effortLevel: null,
    originalEffortLevel: "high",
  });
  assert.deepEqual(submission, { persist: true, level: null });
});
