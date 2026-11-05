import assert from "node:assert/strict";
import test from "node:test";

const AGENT = "a".repeat(64);
const OWNER = "b".repeat(64);

const calls = [];
let current = { respondTo: "anyone", respondToAllowlist: [] };
let saveFailure = null;

const tauriMock = {
  invoke(command, args) {
    calls.push({ command, args });
    if (command === "get_external_agent_access") {
      return Promise.resolve(current);
    }
    if (command === "set_external_agent_access") {
      return saveFailure
        ? Promise.reject(new Error(saveFailure))
        : Promise.resolve();
    }
    if (command === "get_agent_access_owner_only") {
      return Promise.resolve(false);
    }
    return new Promise(() => {});
  },
  transformCallback() {
    return Math.random();
  },
};
globalThis.__TAURI_INTERNALS__ = tauriMock;
globalThis.window.__TAURI_INTERNALS__ = tauriMock;
globalThis.window.matchMedia ??= () => ({
  addEventListener() {},
  matches: false,
  removeEventListener() {},
});
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};

const React = (await import("react")).default;
const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { QueryClient, QueryClientProvider } = await import(
  "@tanstack/react-query"
);
const { ThemeProvider } = await import("@/shared/theme/ThemeProvider");
const { EditExternalAccessDialog } = await import(
  "./EditExternalAccessDialog.tsx"
);

async function settle() {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 10));
  });
}

async function mountDialog() {
  calls.length = 0;
  const closed = [];
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(
      React.createElement(
        QueryClientProvider,
        { client: queryClient },
        React.createElement(
          ThemeProvider,
          null,
          React.createElement(EditExternalAccessDialog, {
            agent: { pubkey: AGENT, name: "Hermes" },
            currentPubkey: OWNER,
            onOpenChange: (open) => closed.push(open),
          }),
        ),
      ),
    );
  });
  await settle();
  return {
    closed,
    select: () =>
      document.querySelector("[data-testid='agent-respond-to-select']"),
    saveButton: () =>
      document.querySelector("[data-testid='external-agent-access-save']"),
    unmount: async () => {
      await act(async () => root.unmount());
      container.remove();
    },
  };
}

async function choose(select, value) {
  const setter = Object.getOwnPropertyDescriptor(
    window.HTMLSelectElement.prototype,
    "value",
  ).set;
  await act(async () => {
    setter.call(select, value);
    select.dispatchEvent(new Event("change", { bubbles: true }));
  });
}

function saves() {
  return calls.filter(({ command }) => command === "set_external_agent_access");
}

test("opens on the saved setting and saves the new one", async () => {
  current = { respondTo: "anyone", respondToAllowlist: [] };
  saveFailure = null;
  const dialog = await mountDialog();
  assert.equal(dialog.select().value, "anyone");

  await choose(dialog.select(), "owner-only");
  await act(async () => dialog.saveButton().click());
  await settle();

  assert.equal(saves().length, 1);
  assert.deepEqual(saves()[0].args, {
    pubkey: AGENT,
    name: "Hermes",
    respondTo: "owner-only",
    allowlist: [],
  });
  assert.deepEqual(dialog.closed, [false]);
  await dialog.unmount();
});

test("a refused save stays open and says why", async () => {
  current = { respondTo: "owner-only", respondToAllowlist: [] };
  saveFailure = "relay said no";
  const dialog = await mountDialog();

  await choose(dialog.select(), "anyone");
  await act(async () => dialog.saveButton().click());
  await settle();

  assert.deepEqual(dialog.closed, []);
  assert.match(document.body.textContent, /relay said no/);
  await dialog.unmount();
});

test("an allowlist needs someone before it can save", async () => {
  current = { respondTo: "owner-only", respondToAllowlist: [] };
  saveFailure = null;
  const dialog = await mountDialog();

  await choose(dialog.select(), "allowlist");

  assert.equal(dialog.saveButton().disabled, true);
  await dialog.unmount();
});
