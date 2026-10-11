import assert from "node:assert/strict";
import test from "node:test";

const { fireEvent } = await import("@testing-library/react");
const React = (await import("react")).default;
const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { CodeWorkField } = await import("./CodeWorkField.tsx");
const { CODE_WORK_OFF_ENV_KEY } = await import("./codeWorkSetting.ts");

// Mount the real checkbox over a stateful env map, as the agent dialogs do.
async function mount({ envVars = {}, inheritedEnvVars } = {}) {
  const latest = { envVars };
  function Harness() {
    const [value, setValue] = React.useState(envVars);
    latest.envVars = value;
    return React.createElement(CodeWorkField, {
      disabled: false,
      envVars: value,
      id: "code-work",
      inheritedEnvVars,
      onEnvVarsChange: setValue,
    });
  }
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  await act(async () => root.render(React.createElement(Harness)));
  const checkbox = container.querySelector("#code-work");
  const unmount = () => {
    act(() => root.unmount());
    container.remove();
  };
  return { checkbox, latest, unmount };
}

test("Code work is on when the setting is missing and round-trips off", async () => {
  const { checkbox, latest, unmount } = await mount({ envVars: { KEEP: "1" } });
  assert.equal(checkbox.checked, true, "missing setting means on");
  assert.match(checkbox.labels[0].textContent, /Code work/);

  await act(async () => fireEvent.click(checkbox));
  assert.equal(checkbox.checked, false);
  assert.deepEqual(latest.envVars, {
    KEEP: "1",
    [CODE_WORK_OFF_ENV_KEY]: "true",
  });

  await act(async () => fireEvent.click(checkbox));
  assert.equal(checkbox.checked, true);
  assert.deepEqual(latest.envVars, { KEEP: "1" }, "on is stored as absent");
  unmount();
});

test("Code work reads a stored off and an inherited off", async () => {
  const stored = await mount({ envVars: { [CODE_WORK_OFF_ENV_KEY]: "true" } });
  assert.equal(stored.checkbox.checked, false);
  stored.unmount();

  const inherited = await mount({
    inheritedEnvVars: { [CODE_WORK_OFF_ENV_KEY]: "true" },
  });
  assert.equal(inherited.checkbox.checked, false);
  await act(async () => fireEvent.click(inherited.checkbox));
  assert.equal(inherited.checkbox.checked, true);
  assert.deepEqual(
    inherited.latest.envVars,
    { [CODE_WORK_OFF_ENV_KEY]: "false" },
    "turning on over an inherited off needs an explicit override",
  );
  inherited.unmount();
});
