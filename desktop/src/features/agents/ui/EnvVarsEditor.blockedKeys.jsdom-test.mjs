import assert from "node:assert/strict";
import test from "node:test";

const { fireEvent } = await import("@testing-library/react");
const React = (await import("react")).default;
const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { EnvVarsEditor } = await import("./EnvVarsEditor.tsx");
const { getAllProviderApiKeyEnvVars } = await import(
  "./agentConfigOptions.tsx"
);

const MESSAGE = "API keys go in AI connections › Tools & accounts.";

// The defaults surfaces: provider API keys are hidden (saved elsewhere) and
// blocked as new rows. Mount over a stateful env map like AgentConfigFields.
async function mount(envVars) {
  const blocked = getAllProviderApiKeyEnvVars();
  const latest = { envVars, hasBlockedRows: false };
  function Harness() {
    const [value, setValue] = React.useState(envVars);
    const [hasBlockedRows, setHasBlockedRows] = React.useState(false);
    latest.envVars = value;
    latest.hasBlockedRows = hasBlockedRows;
    return React.createElement(EnvVarsEditor, {
      blockedKeyMessage: MESSAGE,
      blockedKeys: blocked,
      hiddenKeys: blocked,
      onBlockedRowsChange: setHasBlockedRows,
      onChange: setValue,
      value,
    });
  }
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  await act(async () => root.render(React.createElement(Harness)));
  const unmount = () => {
    act(() => root.unmount());
    container.remove();
  };
  return { container, latest, unmount };
}

function keyInputs(container) {
  return [...container.querySelectorAll('[data-testid="env-vars-key"]')];
}

test("provider API key names come from the provider list", () => {
  const keys = getAllProviderApiKeyEnvVars();
  for (const key of [
    "ANTHROPIC_API_KEY",
    "OPENAI_COMPAT_API_KEY",
    "OPENROUTER_API_KEY",
  ]) {
    assert.ok(keys.includes(key), key);
  }
  assert.ok(
    !keys.includes("OPENAI_API_KEY"),
    "card-mint key is not a provider key",
  );
});

test("a typed provider API key row blocks Save, shows the message, and never touches the saved key", async () => {
  const { container, latest, unmount } = await mount({
    ANTHROPIC_API_KEY: "sk-saved",
    KEEP: "1",
  });
  assert.deepEqual(
    keyInputs(container).map((input) => input.value),
    ["KEEP"],
    "saved provider keys stay out of the generic rows",
  );

  await act(async () =>
    fireEvent.click(container.querySelector('[data-testid="env-vars-add"]')),
  );
  const row = keyInputs(container)[1];
  await act(async () =>
    fireEvent.change(row, { target: { value: "ANTHROPIC_API_KEY" } }),
  );
  assert.equal(latest.hasBlockedRows, true);
  assert.equal(
    container.querySelector('[data-testid="env-vars-blocked-key"]')
      ?.textContent,
    MESSAGE,
  );
  assert.deepEqual(latest.envVars, {
    ANTHROPIC_API_KEY: "sk-saved",
    KEEP: "1",
  });

  const removeButtons = container.querySelectorAll(
    '[data-testid="env-vars-remove"]',
  );
  await act(async () => fireEvent.click(removeButtons[1]));
  assert.equal(latest.hasBlockedRows, false);
  assert.equal(
    container.querySelector('[data-testid="env-vars-blocked-key"]'),
    null,
  );
  assert.deepEqual(latest.envVars, {
    ANTHROPIC_API_KEY: "sk-saved",
    KEEP: "1",
  });
  unmount();
});

test("blocking ignores case, like Windows env names", async () => {
  const { container, latest, unmount } = await mount({ KEEP: "1" });
  await act(async () =>
    fireEvent.click(container.querySelector('[data-testid="env-vars-add"]')),
  );
  const row = keyInputs(container)[1];
  await act(async () =>
    fireEvent.change(row, { target: { value: "anthropic_api_key" } }),
  );
  assert.equal(latest.hasBlockedRows, true);
  assert.deepEqual(latest.envVars, { KEEP: "1" });
  unmount();
});
