import assert from "node:assert/strict";
import test from "node:test";

const values = new Map();
globalThis.localStorage = {
  getItem: (key) => values.get(key) ?? null,
  setItem: (key, value) => values.set(key, String(value)),
};
// A corrupted stored value must not break module load or flip the default.
values.set("buzz-default-ai-auto-join", "{not json");

const preference = await import("./defaultAiPreferences.ts");

test("auto-join defaults to on, including over missing or broken storage", () => {
  assert.equal(preference.DEFAULT_AI_AUTO_JOIN_DEFAULT, true);
  assert.equal(preference.getDefaultAiAutoJoin(), true);
  assert.equal(preference.parseDefaultAiAutoJoin(null), true);
  assert.equal(preference.parseDefaultAiAutoJoin(undefined), true);
  assert.equal(preference.parseDefaultAiAutoJoin("{not json"), true);
  assert.equal(preference.parseDefaultAiAutoJoin('"false"'), true);
  assert.equal(preference.parseDefaultAiAutoJoin("0"), true);
});

test("only a stored JSON boolean is honoured", () => {
  assert.equal(preference.parseDefaultAiAutoJoin("false"), false);
  assert.equal(preference.parseDefaultAiAutoJoin("true"), true);
});

test("changing the preference persists it under the stable key", () => {
  preference.setDefaultAiAutoJoin(false);
  assert.equal(preference.getDefaultAiAutoJoin(), false);
  assert.equal(
    values.get(preference.DEFAULT_AI_AUTO_JOIN_STORAGE_KEY),
    "false",
  );

  preference.setDefaultAiAutoJoin(true);
  assert.equal(preference.getDefaultAiAutoJoin(), true);
  assert.equal(values.get(preference.DEFAULT_AI_AUTO_JOIN_STORAGE_KEY), "true");
});
