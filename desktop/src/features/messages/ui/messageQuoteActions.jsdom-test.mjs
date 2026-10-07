import assert from "node:assert/strict";
import test from "node:test";

// Quote / Reply in thread actions on the message action bar, and the quote
// chip above the composer. Rows only offer Quote inside a conversation scope
// (main timeline or open thread) whose composer can carry the quote.

const tauriMock = {
  invoke() {
    return Promise.resolve([]);
  },
  transformCallback() {
    return Math.random();
  },
};
globalThis.__TAURI_INTERNALS__ = tauriMock;
globalThis.window.__TAURI_INTERNALS__ = tauriMock;
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
const { MessageActionBar } = await import("./MessageActionBar.tsx");
const { ComposerReplyEditBanner } = await import(
  "./ComposerReplyEditBanner.tsx"
);
const { MessageQuoteScope } = await import("./messageQuoteScope.tsx");

const EVENT_ID = "a".repeat(64);
const AUTHOR = "b".repeat(64);

function timelineMessage(overrides = {}) {
  return {
    author: "Alice",
    body: "Ship the launch plan",
    createdAt: 1,
    depth: 0,
    id: EVENT_ID,
    pubkey: AUTHOR,
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
        React.createElement(TooltipProvider, null, element),
      ),
    );
  });
  return {
    container,
    unmount: async () => {
      await act(async () => root.unmount());
      container.remove();
    },
  };
}

function actionLabels(container) {
  return [...container.querySelectorAll("button")].map((button) =>
    button.getAttribute("aria-label"),
  );
}

test("Reply in thread is followed by Quote inside a quote scope", async () => {
  const quoted = [];
  const replied = [];
  const message = timelineMessage();
  const { container, unmount } = await render(
    React.createElement(
      MessageQuoteScope,
      {
        value: {
          cancel() {},
          quote: (target) => quoted.push(target),
          target: null,
        },
      },
      React.createElement(MessageActionBar, {
        message,
        onReply: (target) => replied.push(target),
        reactions: [],
      }),
    ),
  );
  const labels = actionLabels(container);
  const replyIndex = labels.indexOf("Reply in thread");
  assert.ok(replyIndex >= 0, `Reply in thread missing: ${labels}`);
  assert.equal(labels[replyIndex + 1], "Quote");
  assert.equal(labels.includes("Reply"), false);

  await act(async () => {
    fireEvent.click(
      container.querySelector(`[data-testid='quote-message-${EVENT_ID}']`),
    );
  });
  assert.deepEqual(quoted, [message]);
  assert.deepEqual(replied, []);

  await act(async () => {
    fireEvent.click(
      container.querySelector(`[data-testid='reply-message-${EVENT_ID}']`),
    );
  });
  assert.deepEqual(replied, [message]);
  await unmount();
});

test("Quote is hidden outside a scope and for pending messages", async () => {
  const outside = await render(
    React.createElement(MessageActionBar, {
      message: timelineMessage(),
      onReply() {},
      reactions: [],
    }),
  );
  assert.equal(actionLabels(outside.container).includes("Quote"), false);
  await outside.unmount();

  const pending = await render(
    React.createElement(
      MessageQuoteScope,
      { value: { cancel() {}, quote() {}, target: null } },
      React.createElement(MessageActionBar, {
        message: timelineMessage({ pending: true }),
        onReply() {},
        reactions: [],
      }),
    ),
  );
  assert.equal(actionLabels(pending.container).includes("Quote"), false);
  await pending.unmount();
});

test("the composer banner shows a removable quote chip", async () => {
  let cancelled = 0;
  const { container, unmount } = await render(
    React.createElement(ComposerReplyEditBanner, {
      isEditing: false,
      onCancelQuote: () => {
        cancelled += 1;
      },
      quoteTarget: {
        author: "Alice",
        authorPubkey: AUTHOR,
        eventId: EVENT_ID,
        excerpt: "Ship the launch plan",
      },
    }),
  );
  const chip = container.querySelector("[data-testid='quote-target']");
  assert.ok(chip);
  assert.match(chip.textContent, /Quoting Alice/);
  assert.match(chip.textContent, /Ship the launch plan/);
  await act(async () => {
    fireEvent.click(container.querySelector("[aria-label='Cancel quote']"));
  });
  assert.equal(cancelled, 1);
  await unmount();
});

test("editing hides the quote chip; a reply target and quote show together", async () => {
  const quoteTarget = {
    author: "Alice",
    authorPubkey: AUTHOR,
    eventId: EVENT_ID,
    excerpt: "Ship it",
  };
  const editing = await render(
    React.createElement(ComposerReplyEditBanner, {
      isEditing: true,
      quoteTarget,
    }),
  );
  assert.equal(
    editing.container.querySelector("[data-testid='quote-target']"),
    null,
  );
  await editing.unmount();

  const both = await render(
    React.createElement(ComposerReplyEditBanner, {
      isEditing: false,
      quoteTarget,
      replyTarget: { author: "Bob", body: "hi", id: "c".repeat(64) },
    }),
  );
  assert.ok(both.container.querySelector("[data-testid='reply-target']"));
  assert.ok(both.container.querySelector("[data-testid='quote-target']"));
  await both.unmount();
});
