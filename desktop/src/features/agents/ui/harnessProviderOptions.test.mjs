import assert from "node:assert/strict";
import test from "node:test";

import {
  HARNESS_UNAVAILABLE_MODEL_DESCRIPTION,
  harnessProviderOfModel,
  modelForHarnessProvider,
  modelOptionsForHarnessProvider,
} from "./harnessProviderOptions.ts";
import { getDiscoveredPersonaModelOptions } from "./usePersonaModelDiscovery.ts";

const providers = [
  {
    id: "openai-codex",
    label: "ChatGPT or Codex Subscription",
    isDefault: true,
  },
  { id: "openrouter", label: "OpenRouter", isDefault: false },
  { id: "custom:local", label: "local", isDefault: false },
];

const options = [
  { id: "", label: "Default model" },
  { id: "openai-codex:gpt-6-sol", label: "gpt-6-sol" },
  { id: "openrouter:anthropic/claude-opus-5", label: "claude-opus-5" },
  { id: "openrouter:openai/gpt-6", label: "gpt-6" },
  { id: "custom:local:llama3:8b", label: "llama3:8b" },
];

test("no model belongs to the harness default provider", () => {
  assert.equal(harnessProviderOfModel("", providers), "openai-codex");
});

test("models with extra colons map to their full provider id", () => {
  assert.equal(
    harnessProviderOfModel("custom:local:llama3:8b", providers),
    "custom:local",
  );
  assert.equal(
    harnessProviderOfModel("openrouter:anthropic/claude-opus-5", providers),
    "openrouter",
  );
});

test("a saved model of no listed provider shows under the default provider", () => {
  assert.equal(
    harnessProviderOfModel("anthropic:claude-opus-4", providers),
    "openai-codex",
  );
});

test("the default row is offered only for the default provider", () => {
  assert.deepEqual(
    modelOptionsForHarnessProvider(options, providers, "openai-codex").map(
      (option) => option.id,
    ),
    ["", "openai-codex:gpt-6-sol"],
  );
  assert.deepEqual(
    modelOptionsForHarnessProvider(options, providers, "openrouter").map(
      (option) => option.id,
    ),
    ["openrouter:anthropic/claude-opus-5", "openrouter:openai/gpt-6"],
  );
});

test("picking a provider stores the harness default or that provider's first model", () => {
  assert.equal(modelForHarnessProvider("openai-codex", options, providers), "");
  assert.equal(
    modelForHarnessProvider("openrouter", options, providers),
    "openrouter:anthropic/claude-opus-5",
  );
  assert.equal(
    modelForHarnessProvider("custom:local", options, providers),
    "custom:local:llama3:8b",
  );
});

test("a saved model the harness no longer offers is labelled as such", () => {
  const discovered = getDiscoveredPersonaModelOptions(
    {
      agentName: "hermes-agent",
      agentVersion: "0.21.1",
      models: [
        { id: "openai-codex:gpt-6-sol", name: null, description: null },
        {
          id: "anthropic:claude-opus-4",
          name: null,
          description: HARNESS_UNAVAILABLE_MODEL_DESCRIPTION,
        },
      ],
      agentDefaultModel: null,
      selectedModel: "anthropic:claude-opus-4",
      supportsSwitching: true,
      harnessProviders: providers.slice(0, 1),
    },
    "",
  );
  const saved = discovered.find(
    (option) => option.id === "anthropic:claude-opus-4",
  );
  assert.match(saved.label, /not available in Hermes/);
});
