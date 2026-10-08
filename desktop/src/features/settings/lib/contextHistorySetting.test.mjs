import assert from "node:assert/strict";
import test from "node:test";

import {
  CONTEXT_HISTORY_BUDGET_OPTIONS,
  contextHistoryBudgetDescription,
  contextHistoryEnvValue,
  DEFAULT_CONTEXT_HISTORY_SETTING,
  selectContextHistoryBudget,
  selectContextHistoryMode,
} from "./contextHistorySetting.ts";

test("default is the recent messages, like the Rust default", () => {
  assert.deepEqual(DEFAULT_CONTEXT_HISTORY_SETTING, {
    mode: "recent",
    budget: "medium",
  });
  assert.equal(contextHistoryEnvValue(DEFAULT_CONTEXT_HISTORY_SETTING), "");
});

test("budget mode sends the budget; recent ignores it", () => {
  assert.equal(
    contextHistoryEnvValue({ mode: "budget", budget: "large" }),
    "large",
  );
  assert.equal(contextHistoryEnvValue({ mode: "recent", budget: "large" }), "");
});

test("switching modes keeps the chosen budget", () => {
  assert.deepEqual(
    selectContextHistoryMode({ mode: "budget", budget: "small" }, "recent"),
    { mode: "recent", budget: "small" },
  );
  assert.deepEqual(
    selectContextHistoryMode({ mode: "recent", budget: "small" }, "budget"),
    { mode: "budget", budget: "small" },
  );
});

test("picking a budget switches to full history", () => {
  assert.deepEqual(selectContextHistoryBudget("large"), {
    mode: "budget",
    budget: "large",
  });
});

test("budgets are listed small to large with their sizes", () => {
  assert.deepEqual(
    CONTEXT_HISTORY_BUDGET_OPTIONS.map((option) => option.value),
    ["small", "medium", "large"],
  );
  assert.match(
    contextHistoryBudgetDescription({ mode: "budget", budget: "small" }),
    /8k tokens/,
  );
  assert.doesNotMatch(
    contextHistoryBudgetDescription({ mode: "recent", budget: "small" }),
    /8k tokens/,
  );
});
