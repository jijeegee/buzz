/**
 * Mounted gating pins for `ModelEffortFields`, the one place a model control
 * and its thinking-effort companion are paired.
 *
 * The component owns no gating logic of its own: it renders the surface's
 * model control (`children`) and hands `effort` to the shared
 * `EffortPickerField`, which decides visibility through `effortPickerState`
 * (local backend AND a vocabulary). These tests pin that pairing against the
 * real picker so a surface that adopts the wrapper gets exactly the
 * create/edit gating — and that `effort: null` is the only way to opt out.
 */

import assert from "node:assert/strict";
import { afterEach, before, test } from "node:test";
import { JSDOM } from "jsdom";

const dom = new JSDOM("<!doctype html><html><body></body></html>", {
  url: "http://localhost",
});

Object.assign(globalThis, {
  document: dom.window.document,
  window: dom.window,
  IS_REACT_ACT_ENVIRONMENT: true,
  localStorage: dom.window.localStorage,
  self: dom.window,
  ResizeObserver: class {
    observe() {}
    unobserve() {}
    disconnect() {}
  },
});
Object.defineProperty(globalThis, "navigator", {
  configurable: true,
  value: dom.window.navigator,
  writable: true,
});
for (const key of Object.getOwnPropertyNames(dom.window)) {
  if (key === "window" || key === "document" || key === "globalThis") continue;
  const value = dom.window[key];
  if (
    typeof value === "function" &&
    /^(HTML|SVG)|Element$|Event$|EventTarget$|^Node|^Document|Observer$/.test(
      key,
    )
  ) {
    globalThis[key] = value;
  }
}
globalThis.getComputedStyle = dom.window.getComputedStyle.bind(dom.window);
dom.window.HTMLElement.prototype.hasPointerCapture = () => false;
dom.window.HTMLElement.prototype.releasePointerCapture = () => {};
dom.window.HTMLElement.prototype.scrollIntoView = () => {};

let act;
let cleanup;
let render;
let createElement;
let ModelEffortFields;
let EFFORT_TEMPLATE_DEFAULT_LABEL;

before(async () => {
  ({ act, cleanup, render } = await import("@testing-library/react"));
  ({ createElement } = await import("react"));
  ({ ModelEffortFields } = await import("./ModelEffortFields.tsx"));
  ({ EFFORT_TEMPLATE_DEFAULT_LABEL } = await import("./effortPicker.ts"));
});

afterEach(() => {
  cleanup?.();
});

// What the Rust catalog publishes for Claude Code / Codex / Hermes before any
// session has discovered the real option list.
const claudeRuntime = {
  effortThoughtLevel: {
    configOptionId: "effort",
    fallbackValues: ["low", "medium", "high"],
  },
};
// Goose / buzz-agent / custom harnesses publish no ACP effort option.
const gooseRuntime = { effortThoughtLevel: null };

function modelControl() {
  return createElement(
    "div",
    { "data-testid": "model-control", id: "model-control" },
    "Model",
  );
}

async function mount(props) {
  await act(async () => {
    render(
      createElement(
        ModelEffortFields,
        { disabled: false, ...props },
        modelControl(),
      ),
    );
  });
  const doc = dom.window.document;
  return {
    model: doc.getElementById("model-control"),
    effort: doc.getElementById("edit-agent-effort"),
    wrapper: doc.querySelector('[data-testid="model-effort-fields"]'),
  };
}

function effortFor(overrides = {}) {
  return {
    backend: { type: "local" },
    config: undefined,
    onChange: () => {},
    runtime: claudeRuntime,
    value: null,
    ...overrides,
  };
}

test("a local model surface on an effort-capable harness renders the model control with the effort companion beneath it", async () => {
  const { model, effort, wrapper } = await mount({ effort: effortFor() });
  assert.ok(model, "the surface's own model control renders");
  assert.ok(effort, "the shared effort picker renders alongside it");
  assert.ok(wrapper, "both live in the one pairing wrapper");
  assert.ok(
    model.compareDocumentPosition(effort) &
      dom.window.Node.DOCUMENT_POSITION_FOLLOWING,
    "effort follows the model control",
  );
});

test("a provider backend keeps the model control but never offers effort", async () => {
  // Remote effort is deploy-time policy_env; the Rust update rejects a write.
  const { model, effort } = await mount({
    effort: effortFor({
      backend: { type: "provider", id: "blox", config: {} },
    }),
  });
  assert.ok(model);
  assert.equal(effort, null);
});

test("a harness with no effort vocabulary keeps the model control alone", async () => {
  const { model, effort } = await mount({
    effort: effortFor({ runtime: gooseRuntime }),
  });
  assert.ok(model);
  assert.equal(effort, null, "no catalog fallback and no session option");
});

test("a running session's discovered option is enough even without a catalog fallback", async () => {
  const { effort } = await mount({
    effort: effortFor({
      runtime: gooseRuntime,
      config: {
        effortConfigId: "thought_level",
        effortOptions: [{ value: "low", displayName: "Low" }],
      },
    }),
  });
  assert.ok(effort, "the discovered vocabulary wins over the missing fallback");
});

test("effort: null is the explicit opt-out — the model control renders alone", async () => {
  const { model, effort } = await mount({ effort: null });
  assert.ok(model);
  assert.equal(effort, null);
});

test("the companion shows the current effort, honours the surface's sentinel label, and shares the save gate", async () => {
  const { effort } = await mount({
    disabled: true,
    effort: effortFor({
      defaultLabel: EFFORT_TEMPLATE_DEFAULT_LABEL,
      value: null,
    }),
  });
  assert.ok(effort);
  assert.ok(
    effort.textContent?.includes("Template default"),
    `a linked instance's sentinel names the template tier; got "${effort.textContent}"`,
  );
  assert.equal(effort.disabled, true, "disabled flows from the wrapper");

  cleanup();
  const seeded = await mount({ effort: effortFor({ value: "high" }) });
  assert.ok(
    seeded.effort.textContent?.includes("High"),
    `the stored effort preselects its option; got "${seeded.effort.textContent}"`,
  );
});

test("a definition or global surface passes no backend and gates on vocabulary alone", async () => {
  const { effort } = await mount({
    effort: effortFor({ backend: undefined }),
  });
  assert.ok(
    effort,
    "no instance → no local gate; the catalog vocabulary is enough",
  );
  cleanup();
  const bare = await mount({
    effort: effortFor({ backend: undefined, runtime: gooseRuntime }),
  });
  assert.equal(
    bare.effort,
    null,
    "…but still nothing to pick without a vocabulary",
  );
});
