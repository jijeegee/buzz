import assert from "node:assert/strict";
import test from "node:test";

import {
  buildProviderRows,
  defaultOptions,
  groupProviderRows,
  nextGlobalConfigForApiKey,
  nextGlobalConfigForDefault,
  saveNoticeText,
} from "./providersSettingsModel.ts";

// ── Fixtures ─────────────────────────────────────────────────────────────────

function runtime(overrides) {
  return {
    id: "custom",
    label: "Custom",
    avatarUrl: "",
    availability: "available",
    command: "custom",
    binaryPath: "/bin/custom",
    defaultArgs: [],
    mcpCommand: null,
    modelEnvVar: null,
    providerEnvVar: null,
    thinkingEnvVar: null,
    effortCanonicalValues: null,
    effortThoughtLevel: null,
    maxTokensEnvVar: null,
    contextLimitEnvVar: null,
    maxRoundsEnvVar: null,
    installHint: "",
    installInstructionsUrl: "",
    canAutoInstall: false,
    requiresExternalCli: false,
    underlyingCliPath: null,
    nodeRequired: false,
    authStatus: { status: "not_applicable" },
    loginHint: null,
    subscriptionProvider: null,
    source: "builtin",
    ...overrides,
  };
}

const claude = runtime({
  id: "claude",
  label: "Claude Code",
  subscriptionProvider: "anthropic",
  authStatus: { status: "logged_in" },
});
const codex = runtime({
  id: "codex",
  label: "Codex",
  subscriptionProvider: "openai",
  availability: "not_installed",
  command: null,
  binaryPath: null,
});
const buzzAgent = runtime({
  id: "buzz-agent",
  label: "Buzz Agent",
  modelEnvVar: "BUZZ_AGENT_MODEL",
  providerEnvVar: "BUZZ_AGENT_PROVIDER",
});
const goose = runtime({
  id: "goose",
  label: "Goose",
  modelEnvVar: "GOOSE_MODEL",
  providerEnvVar: "GOOSE_PROVIDER",
});
const catalog = [goose, claude, codex, buzzAgent];

function config(overrides) {
  return {
    env_vars: {},
    provider: null,
    model: null,
    preferred_runtime: null,
    effort_level: null,
    ...overrides,
  };
}

const rowFor = (rows, id) => rows.find((row) => row.providerId === id);

// ── buildProviderRows ────────────────────────────────────────────────────────

test("subscription runtimes are matched only through subscriptionProvider", () => {
  const rows = buildProviderRows(catalog, config());
  assert.equal(rowFor(rows, "anthropic").subscriptionRuntime, claude);
  assert.equal(rowFor(rows, "openai").subscriptionRuntime, codex);
  assert.equal(rowFor(rows, "openrouter").subscriptionRuntime, null);

  // A runtime that *looks* like Claude but publishes no fact is not matched —
  // the id is never consulted.
  const unlabeled = runtime({ id: "claude", label: "Claude Code" });
  const rowsWithoutFact = buildProviderRows([unlabeled, buzzAgent], config());
  assert.equal(rowFor(rowsWithoutFact, "anthropic").subscriptionRuntime, null);
});

test("rows are listed in a fixed order and carry their credential facts", () => {
  const rows = buildProviderRows(catalog, config());
  assert.deepEqual(
    rows.map((row) => row.providerId),
    [
      "anthropic",
      "openai",
      "openai-compat",
      "openrouter",
      "databricks",
      "databricks_v2",
    ],
  );
  assert.equal(rowFor(rows, "anthropic").apiKeyEnvVar, "ANTHROPIC_API_KEY");
  assert.equal(rowFor(rows, "anthropic").apiKeyLabel, "Anthropic API Key");
  assert.equal(rowFor(rows, "openai").apiKeyEnvVar, "OPENAI_COMPAT_API_KEY");
  assert.equal(rowFor(rows, "openai-compat").sharesApiKeyWith, "openai");
  assert.equal(rowFor(rows, "openai").sharesApiKeyWith, null);
  assert.equal(rowFor(rows, "databricks").apiKeyEnvVar, null);
  assert.equal(rowFor(rows, "databricks").sharesApiKeyWith, null);
  assert.equal(rowFor(rows, "databricks_v2").sharesApiKeyWith, null);
});

test("hidden provider ids are skipped", () => {
  const rows = buildProviderRows(catalog, config(), new Set(["databricks"]));
  assert.equal(rowFor(rows, "databricks"), undefined);
  assert.ok(rowFor(rows, "databricks_v2"));
});

test("apiKeyIsSet reflects a non-empty global env value only", () => {
  const unset = buildProviderRows(catalog, config());
  assert.equal(rowFor(unset, "anthropic").apiKeyIsSet, false);
  const blank = buildProviderRows(
    catalog,
    config({ env_vars: { ANTHROPIC_API_KEY: "" } }),
  );
  assert.equal(rowFor(blank, "anthropic").apiKeyIsSet, false);
  const set = buildProviderRows(
    catalog,
    config({ env_vars: { ANTHROPIC_API_KEY: "sk-ant" } }),
  );
  assert.equal(rowFor(set, "anthropic").apiKeyIsSet, true);
  // The OpenAI key is shared with OpenAI-compatible: both rows read it.
  const openai = buildProviderRows(
    catalog,
    config({ env_vars: { OPENAI_COMPAT_API_KEY: "sk" } }),
  );
  assert.equal(rowFor(openai, "openai").apiKeyIsSet, true);
  assert.equal(rowFor(openai, "openai-compat").apiKeyIsSet, true);
});

test("the API-key harness is an available provider-selection runtime", () => {
  // Preferred runtime wins when it takes a provider.
  const preferGoose = buildProviderRows(
    catalog,
    config({ preferred_runtime: "goose" }),
  );
  assert.equal(rowFor(preferGoose, "anthropic").apiKeyRuntime, goose);
  // A preferred login harness does not take a provider: picker order applies.
  const preferClaude = buildProviderRows(
    catalog,
    config({ preferred_runtime: "claude" }),
  );
  assert.equal(rowFor(preferClaude, "anthropic").apiKeyRuntime, buzzAgent);
  // Nothing installed that takes a provider → no API-key harness.
  const none = buildProviderRows([claude, codex], config());
  assert.equal(rowFor(none, "anthropic").apiKeyRuntime, null);
  // An uninstalled provider runtime is never chosen.
  const uninstalled = buildProviderRows(
    [claude, runtime({ ...buzzAgent, availability: "not_installed" })],
    config(),
  );
  assert.equal(rowFor(uninstalled, "anthropic").apiKeyRuntime, null);
});

test("defaultPath: at most one row across the table is selected", () => {
  const cases = [
    { cfg: config(), expect: {} },
    {
      cfg: config({ preferred_runtime: "claude" }),
      expect: { anthropic: "subscription" },
    },
    {
      // provider is ignored by a login harness
      cfg: config({ preferred_runtime: "claude", provider: "openai" }),
      expect: { anthropic: "subscription" },
    },
    {
      cfg: config({ preferred_runtime: "codex" }),
      expect: { openai: "subscription" },
    },
    {
      cfg: config({ preferred_runtime: "buzz-agent", provider: "anthropic" }),
      expect: { anthropic: "api-key" },
    },
    {
      cfg: config({ preferred_runtime: null, provider: "openrouter" }),
      expect: { openrouter: "api-key" },
    },
    {
      cfg: config({ preferred_runtime: "goose", provider: "openai-compat" }),
      expect: { "openai-compat": "api-key" },
    },
    {
      // provider set but a non-provider harness preferred → nothing selected
      cfg: config({ preferred_runtime: "custom", provider: "anthropic" }),
      expect: {},
    },
  ];
  for (const { cfg, expect } of cases) {
    const rows = buildProviderRows([...catalog, runtime()], cfg);
    const selected = Object.fromEntries(
      rows
        .filter((row) => row.defaultPath !== null)
        .map((row) => [row.providerId, row.defaultPath]),
    );
    assert.deepEqual(selected, expect, JSON.stringify(cfg));
    assert.ok(Object.keys(selected).length <= 1);
  }
});

// ── groupProviderRows / defaultOptions ───────────────────────────────────────

test("key-sharing rows fold under the key owner", () => {
  const groups = groupProviderRows(buildProviderRows(catalog, config()));
  assert.deepEqual(
    groups.map((group) => [
      group.primary.providerId,
      group.companions.map((row) => row.providerId),
    ]),
    [
      ["anthropic", []],
      ["openai", ["openai-compat"]],
      ["openrouter", []],
      ["databricks", []],
      ["databricks_v2", []],
    ],
  );
});

test("defaultOptions lists every provider × path with a harness, one selected", () => {
  const rows = buildProviderRows(
    catalog,
    config({ preferred_runtime: "claude" }),
  );
  const options = defaultOptions(rows);
  assert.deepEqual(
    options.map((option) => [option.id, option.available, option.selected]),
    [
      ["anthropic:subscription", true, true],
      ["anthropic:api-key", true, false],
      ["openai:subscription", false, false],
      ["openai:api-key", true, false],
      ["openai-compat:api-key", true, false],
      ["openrouter:api-key", true, false],
      ["databricks:api-key", true, false],
      ["databricks_v2:api-key", true, false],
    ],
  );
  assert.equal(options.filter((option) => option.selected).length, 1);
  assert.equal(options[0].label, "Anthropic · Subscription");
  assert.equal(options[0].detail, "Claude Code sign-in");
  assert.equal(options[2].detail, "Codex sign-in — not installed");
  assert.equal(options[1].detail, "Buzz Agent with ANTHROPIC_API_KEY");
  assert.equal(options[6].label, "Databricks · Workspace");
  assert.equal(options[6].detail, "Buzz Agent with workspace sign-in");

  const noProviderHarness = defaultOptions(
    buildProviderRows([claude], config()),
  );
  assert.equal(noProviderHarness[1].available, false);
  assert.equal(
    noProviderHarness[1].detail,
    "Needs an installed runtime that takes a provider",
  );
});

// ── nextGlobalConfigForDefault ───────────────────────────────────────────────

test("choosing a subscription switches the harness and clears model/effort, never sets a model", () => {
  const before = config({
    preferred_runtime: "buzz-agent",
    provider: "anthropic",
    model: "claude-sonnet",
    effort_level: "high",
    env_vars: { ANTHROPIC_API_KEY: "sk", BUZZ_AGENT_THINKING_EFFORT: "high" },
  });
  const rows = buildProviderRows(catalog, before);
  const next = nextGlobalConfigForDefault(
    before,
    rowFor(rows, "anthropic"),
    "subscription",
  );
  assert.equal(next.preferred_runtime, "claude");
  assert.equal(next.model, null);
  assert.equal(next.effort_level, null);
  // A login harness takes no provider — same clearing as Agent defaults.
  assert.equal(next.provider, null);
  // The key stays: it is still the API-key path's credential.
  assert.equal(next.env_vars.ANTHROPIC_API_KEY, "sk");
  assert.equal("BUZZ_AGENT_THINKING_EFFORT" in next.env_vars, false);
  assert.equal(before.model, "claude-sonnet", "input is not mutated");
});

test("choosing the API-key path keeps the harness when it already takes a provider", () => {
  const before = config({
    preferred_runtime: "goose",
    provider: "anthropic",
    model: "claude-sonnet",
    effort_level: "low",
  });
  const rows = buildProviderRows(catalog, before);
  const next = nextGlobalConfigForDefault(
    before,
    rowFor(rows, "openrouter"),
    "api-key",
  );
  assert.equal(next.preferred_runtime, "goose");
  assert.equal(next.provider, "openrouter");
  // The model belonged to the previous provider; it is cleared, never chosen.
  assert.equal(next.model, null);
  // Same harness: the effort column survives.
  assert.equal(next.effort_level, "low");
});

test("choosing the API-key path from a login harness switches to the provider harness", () => {
  const before = config({ preferred_runtime: "claude", model: "opus" });
  const rows = buildProviderRows(catalog, before);
  const next = nextGlobalConfigForDefault(
    before,
    rowFor(rows, "openai"),
    "api-key",
  );
  assert.equal(next.preferred_runtime, "buzz-agent");
  assert.equal(next.provider, "openai");
  assert.equal(next.model, null);
});

test("re-choosing the current default is a no-op and a missing harness yields null", () => {
  const current = config({ preferred_runtime: "claude" });
  const rows = buildProviderRows(catalog, current);
  assert.equal(
    nextGlobalConfigForDefault(
      current,
      rowFor(rows, "anthropic"),
      "subscription",
    ),
    null,
  );
  const sameProvider = config({
    preferred_runtime: "buzz-agent",
    provider: "openai",
  });
  const sameRows = buildProviderRows(catalog, sameProvider);
  assert.equal(
    nextGlobalConfigForDefault(
      sameProvider,
      rowFor(sameRows, "openai"),
      "api-key",
    ),
    null,
  );
  const noHarness = buildProviderRows([claude], config());
  assert.equal(
    nextGlobalConfigForDefault(
      config(),
      rowFor(noHarness, "anthropic"),
      "api-key",
    ),
    null,
  );
  assert.equal(
    nextGlobalConfigForDefault(
      config(),
      rowFor(noHarness, "openrouter"),
      "subscription",
    ),
    null,
  );
});

test("every default change leaves model unset", () => {
  const rows = buildProviderRows(catalog, config({ model: "stale" }));
  for (const row of rows) {
    for (const path of ["subscription", "api-key"]) {
      const next = nextGlobalConfigForDefault(
        config({ model: "stale", preferred_runtime: "custom" }),
        row,
        path,
      );
      if (next) assert.equal(next.model, null, `${row.providerId}:${path}`);
    }
  }
});

// ── nextGlobalConfigForApiKey / saveNoticeText ───────────────────────────────

test("an API-key save touches only that env var", () => {
  const before = config({
    preferred_runtime: "buzz-agent",
    provider: "anthropic",
    model: "claude-sonnet",
    env_vars: { OPENAI_COMPAT_API_KEY: "keep" },
  });
  const next = nextGlobalConfigForApiKey(
    before,
    "ANTHROPIC_API_KEY",
    " sk-new ",
  );
  assert.deepEqual(next, {
    ...before,
    env_vars: { OPENAI_COMPAT_API_KEY: "keep", ANTHROPIC_API_KEY: "sk-new" },
  });
  // Removal is an empty value; Rust strips it on write.
  assert.equal(
    nextGlobalConfigForApiKey(before, "OPENAI_COMPAT_API_KEY", "").env_vars
      .OPENAI_COMPAT_API_KEY,
    "",
  );
});

test("saveNoticeText mirrors the Agent defaults copy", () => {
  const notice = (restarted_count, failed_restart_count) =>
    saveNoticeText({ config: config(), restarted_count, failed_restart_count });
  assert.equal(notice(0, 0), "Saved.");
  assert.equal(notice(1, 0), "Saved. Restarted 1 agent.");
  assert.equal(notice(3, 0), "Saved. Restarted 3 agents.");
  assert.equal(
    notice(2, 1),
    "Saved. Restarted 2 agents. 1 agent couldn't restart — check the Agents page.",
  );
  assert.equal(
    notice(0, 2),
    "Saved. 2 agents couldn't restart — check the Agents page.",
  );
});
