import assert from "node:assert/strict";
import test from "node:test";

// "Report message" in the More menu: shown for other people's messages only,
// never for my own or an agent's. The report dialog's "Also block" starts off.

const ME = "a".repeat(64);
const OTHER = "b".repeat(64);
const EVENT_ID = "e".repeat(64);

const tauriMock = {
  invoke(cmd) {
    if (cmd === "get_identity") {
      return Promise.resolve({ pubkey: ME, display_name: "Me" });
    }
    // Theme and window chrome calls have nothing to check here.
    return Promise.resolve(cmd.startsWith("get_") ? null : []);
  },
  transformCallback() {
    return Math.random();
  },
};
globalThis.__TAURI_INTERNALS__ = tauriMock;
globalThis.window.__TAURI_INTERNALS__ = tauriMock;
// The report dialog follows the app theme, which reads the color scheme.
window.matchMedia ??= () => ({
  matches: false,
  addEventListener() {},
  removeEventListener() {},
  addListener() {},
  removeListener() {},
});
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};

const { fireEvent } = await import("@testing-library/react");
const React = (await import("react")).default;
const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { QueryClient, QueryClientProvider } = await import(
  "@tanstack/react-query"
);
const { TooltipProvider } = await import("@/shared/ui/tooltip");
const { ThemeProvider } = await import("@/shared/theme/ThemeProvider.tsx");
const { MessageActionBar } = await import("./MessageActionBar.tsx");
const { ReportMessageDialog } = await import(
  "@/features/moderation/ui/ReportMessageDialog.tsx"
);

function timelineMessage(overrides = {}) {
  return {
    author: "Alice",
    body: "hello",
    depth: 0,
    id: EVENT_ID,
    pubkey: OTHER,
    tags: [],
    time: "9:00",
    ...overrides,
  };
}

async function render(element) {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  await act(async () => {
    root.render(
      React.createElement(
        QueryClientProvider,
        { client: queryClient },
        React.createElement(
          ThemeProvider,
          null,
          React.createElement(TooltipProvider, null, element),
        ),
      ),
    );
  });
  // Let the identity query resolve.
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 0));
  });
  return {
    container,
    unmount: async () => {
      await act(async () => root.unmount());
      container.remove();
    },
  };
}

async function reportItemShown(message) {
  const { container, unmount } = await render(
    React.createElement(MessageActionBar, { message, reactions: [] }),
  );
  const trigger = container.querySelector(
    `[data-testid='more-actions-${EVENT_ID}']`,
  );
  assert.ok(trigger, "More actions button missing");
  await act(async () => {
    fireEvent.keyDown(trigger, { key: "Enter" });
  });
  assert.ok(
    document.querySelector("[role='menu']"),
    "More actions menu did not open",
  );
  const shown = Boolean(
    document.querySelector(`[data-testid='report-message-${EVENT_ID}']`),
  );
  await unmount();
  return shown;
}

test("Report is offered on another person's message", async () => {
  assert.equal(await reportItemShown(timelineMessage()), true);
});

test("Report is hidden on my own message", async () => {
  assert.equal(await reportItemShown(timelineMessage({ pubkey: ME })), false);
});

test("Report is hidden on agent messages", async () => {
  assert.equal(
    await reportItemShown(timelineMessage({ isAgent: true })),
    false,
  );
  assert.equal(await reportItemShown(timelineMessage({ role: "bot" })), false);
});

test("the report dialog's Also block starts unchecked", async () => {
  const { unmount } = await render(
    React.createElement(ReportMessageDialog, {
      authorPubkey: OTHER,
      eventId: EVENT_ID,
      onOpenChange() {},
      open: true,
    }),
  );
  const checkbox = document.querySelector("[data-testid='report-also-block']");
  assert.ok(checkbox, "Also block checkbox missing");
  assert.equal(checkbox.getAttribute("aria-checked"), "false");
  await unmount();
});
