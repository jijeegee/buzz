import assert from "node:assert/strict";
import test from "node:test";

const values = new Map();
globalThis.localStorage = {
  getItem: (key) => values.get(key) ?? null,
  setItem: (key, value) => values.set(key, String(value)),
};

const preference = await import("./contextGaugePreference.ts");

test("defaults missing and invalid values to showing the gauge", () => {
  assert.equal(preference.parseContextGaugeEnabled(null), true);
  assert.equal(preference.parseContextGaugeEnabled("invalid"), true);
  assert.equal(preference.parseContextGaugeEnabled("true"), true);
  assert.equal(preference.parseContextGaugeEnabled("false"), false);
});

test("persists changes to the context gauge preference", () => {
  preference.setContextGaugeEnabled(false);
  assert.equal(preference.getContextGaugeEnabled(), false);
  assert.equal(
    values.get(preference.CONTEXT_GAUGE_ENABLED_STORAGE_KEY),
    "false",
  );

  preference.setContextGaugeEnabled(true);
  assert.equal(preference.getContextGaugeEnabled(), true);
  assert.equal(
    values.get(preference.CONTEXT_GAUGE_ENABLED_STORAGE_KEY),
    "true",
  );
});
