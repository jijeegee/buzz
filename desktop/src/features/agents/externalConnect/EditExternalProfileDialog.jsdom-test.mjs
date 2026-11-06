import assert from "node:assert/strict";
import test from "node:test";

const AGENT = "a".repeat(64);

const calls = [];
let saveFailure = null;
let saved = null;
let readFailure = null;
// `null`: the profile read never answers.
let stored = {
  pubkey: AGENT,
  display_name: "Sales",
  avatar_url: "https://example.com/a.png",
  about: "Sells socks",
};

const tauriMock = {
  invoke(command, args) {
    calls.push({ command, args });
    if (command === "get_user_profile") {
      if (readFailure) return Promise.reject(new Error(readFailure));
      return stored === null ? new Promise(() => {}) : Promise.resolve(stored);
    }
    if (command === "set_external_agent_profile") {
      if (saveFailure) return Promise.reject(new Error(saveFailure));
      saved = args;
      return Promise.resolve();
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
const { EditExternalProfileDialog } = await import(
  "./EditExternalProfileDialog.tsx"
);

async function settle() {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 10));
  });
}

async function mountDialog() {
  calls.length = 0;
  saved = null;
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
          React.createElement(EditExternalProfileDialog, {
            agent: { pubkey: AGENT, name: "Hermes" },
            onOpenChange: (open) => closed.push(open),
          }),
        ),
      ),
    );
  });
  await settle();
  const dialog = () =>
    document.querySelector("[data-testid='external-agent-profile-dialog']");
  return {
    closed,
    queryClient,
    nameField: () => dialog().querySelector("#persona-display-name"),
    aboutField: () => dialog().querySelector("#persona-description"),
    saveButton: () =>
      document.querySelector("[data-testid='external-agent-profile-save']"),
    unmount: async () => {
      await act(async () => root.unmount());
      container.remove();
    },
  };
}

async function type(field, value) {
  const setter = Object.getOwnPropertyDescriptor(
    window.HTMLInputElement.prototype,
    "value",
  ).set;
  await act(async () => {
    setter.call(field, value);
    field.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

async function click(button) {
  await act(async () => {
    button.click();
  });
  await settle();
}

test("opens on the stored profile and saves all three fields", async () => {
  saveFailure = null;
  const dialog = await mountDialog();
  assert.equal(dialog.nameField().value, "Sales");
  assert.equal(dialog.aboutField().value, "Sells socks");

  await type(dialog.nameField(), "  Support ");
  await type(dialog.aboutField(), "   ");
  await click(dialog.saveButton());

  assert.deepEqual(saved, {
    pubkey: AGENT,
    displayName: "Support",
    avatarUrl: "https://example.com/a.png",
    about: null,
  });
  assert.deepEqual(dialog.closed, [false]);
  await dialog.unmount();
});

test("saving stays off until the profile is read", async () => {
  const held = stored;
  stored = null;
  const dialog = await mountDialog();
  assert.equal(dialog.saveButton().disabled, true);
  await dialog.unmount();
  stored = held;
});

test("a blank name cannot be saved", async () => {
  const dialog = await mountDialog();
  await type(dialog.nameField(), "   ");
  assert.equal(dialog.saveButton().disabled, true);
  await dialog.unmount();
});

test("a refetch keeps what is being typed", async () => {
  const dialog = await mountDialog();
  await type(dialog.nameField(), "Typing");
  await act(async () => {
    await dialog.queryClient.invalidateQueries({
      queryKey: ["external-agent-profile"],
    });
  });
  await settle();
  assert.equal(dialog.nameField().value, "Typing");
  await dialog.unmount();
});

test("a failed read says so and cannot save", async () => {
  readFailure = "relay offline";
  const dialog = await mountDialog();
  assert.equal(dialog.saveButton().disabled, true);
  assert.match(document.body.textContent, /relay offline/);
  readFailure = null;
  await dialog.unmount();
});

test("a refused save stays open with the reason", async () => {
  saveFailure = "agent_not_found";
  const dialog = await mountDialog();
  await click(dialog.saveButton());
  assert.deepEqual(dialog.closed, []);
  assert.match(document.body.textContent, /agent_not_found/);
  saveFailure = null;
  await dialog.unmount();
});
