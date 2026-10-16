import assert from "node:assert/strict";
import test from "node:test";
import {
  getPersonaProviderOptions,
  getProviderApiKeyEnvVar,
  resetConfigForHarnessChange,
} from "./agentConfigOptions.tsx";
import { selectionOnRuntimeChange } from "./runtimeModelProviderSelection.ts";

test("subscription is offered only by capable runtimes and has no API key", () => {
  for (const supported of [true, false]) {
    const ids = getPersonaProviderOptions(
      "",
      "runtime",
      undefined,
      undefined,
      supported,
    ).map((o) => o.id);
    assert.equal(ids.includes("openai-subscription"), supported);
  }
  assert.equal(getProviderApiKeyEnvVar("openai-subscription"), null);
});

test("global defaults and create/edit clear subscription on unsupported harness switch", () => {
  const config = {
    provider: "openai-subscription",
    model: "gpt-test",
    env_vars: {},
    preferred_runtime: "kuzz-agent",
  };
  assert.equal(
    resetConfigForHarnessChange(config, "goose", false).provider,
    null,
  );
  assert.equal(
    resetConfigForHarnessChange(config, "kuzz-agent", true).provider,
    "openai-subscription",
  );
  const next = selectionOnRuntimeChange(
    {
      provider: config.provider,
      model: config.model,
      envVars: {},
      isCustomModelEditing: false,
      isCustomProviderEditing: false,
    },
    {
      previousRuntime: "kuzz-agent",
      nextRuntime: "goose",
      nextRuntimeCanChooseProvider: true,
      nextRuntimeSupportsChatgptSubscription: false,
      lockedRuntimeReset: "full",
    },
  );
  assert.equal(next.provider, "");
  assert.equal(next.model, "");
});
