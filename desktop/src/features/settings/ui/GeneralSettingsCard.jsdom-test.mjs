import assert from "node:assert/strict";
import test from "node:test";

// Settings › General reads and writes open-at-login through the real Tauri
// command names.

const calls = [];
let enabled = true;
const tauriMock = {
  invoke(command, args) {
    calls.push({ command, args });
    if (command === "get_open_at_login") return Promise.resolve(enabled);
    if (command === "set_open_at_login") {
      enabled = args.enabled;
      return Promise.resolve(null);
    }
    return Promise.resolve(null);
  },
  transformCallback: () => Math.random(),
  unregisterCallback() {},
};
globalThis.__TAURI_INTERNALS__ = tauriMock;
globalThis.window.__TAURI_INTERNALS__ = tauriMock;

const { fireEvent } = await import("@testing-library/react");
const React = (await import("react")).default;
const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { GeneralSettingsCard } = await import("./GeneralSettingsCard.tsx");

async function settle(check) {
  for (let i = 0; i < 50 && !check(); i++) {
    await act(() => new Promise((resolve) => setTimeout(resolve, 10)));
  }
  assert.ok(check());
}

test("open at login shows the saved state and turns off from the switch", async () => {
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  await act(async () => root.render(React.createElement(GeneralSettingsCard)));
  const toggle = () =>
    container.querySelector("[data-testid='settings-open-at-login']");
  await settle(() => toggle()?.getAttribute("aria-checked") === "true");

  await act(async () => fireEvent.click(toggle()));
  await settle(() => toggle().getAttribute("aria-checked") === "false");
  assert.deepEqual(
    calls.filter((c) => c.command === "set_open_at_login").map((c) => c.args),
    [{ enabled: false }],
  );
  await act(async () => root.unmount());
  container.remove();
});
