import assert from "node:assert/strict";
import test from "node:test";

// The thread head goal panel: collapsed it shows the linked goal's path and
// sub-goal progress; expanded it edits the sub-goal tree through the same
// operations as the channel goals panel. The link picker can add a goal and
// link the thread to it as one revision.

const { fireEvent } = await import("@testing-library/react");
const React = (await import("react")).default;
const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { GoalLinkPicker, ThreadGoalPanelView } = await import(
  "./ThreadGoalPanelView.tsx"
);

const THREAD = "ab".repeat(32);

const node = (id, parent, extra = {}) => ({
  id,
  parent,
  title: `Goal ${id}`,
  status: "open",
  order: 0,
  ...extra,
});

const tree = {
  v: 1,
  nodes: [
    node("root", null),
    node("srv", "root", { threads: [THREAD], note: "Keep writes atomic" }),
    node("cas", "srv", { status: "done" }),
    node("val", "srv", { order: 1 }),
    node("ui", "root", { order: 1 }),
  ],
};

async function mount(element) {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  await act(async () => root.render(element));
  return {
    container,
    q: (testId) => container.querySelector(`[data-testid='${testId}']`),
    unmount: async () => {
      await act(async () => root.unmount());
      container.remove();
    },
  };
}

function view(props) {
  const calls = [];
  const element = React.createElement(ThreadGoalPanelView, {
    apply: async (op) => {
      calls.push(op);
    },
    nameOf: () => "Someone",
    threadRootId: THREAD,
    tree,
    ...props,
  });
  return { calls, element };
}

test("a linked thread starts collapsed with its path and progress", async () => {
  const { element } = view();
  const { q, unmount } = await mount(element);
  assert.ok(q("thread-goal-panel"));
  const toggle = q("thread-goal-toggle");
  assert.equal(toggle.getAttribute("aria-expanded"), "false");
  assert.ok(toggle.textContent.includes("Goal srv"));
  assert.ok(
    !toggle.textContent.includes("Goal root"),
    "layer 1 is in the header",
  );
  assert.equal(q("thread-goal-progress").textContent, "1/2");
  assert.equal(q("thread-goal-subtree"), null);
  await unmount();
});

test("expanding shows the sub-goals; status and add use goal operations", async () => {
  const { calls, element } = view();
  const { container, q, unmount } = await mount(element);
  await act(async () => fireEvent.click(q("thread-goal-toggle")));
  assert.equal(q("thread-goal-toggle").getAttribute("aria-expanded"), "true");
  const subtree = q("thread-goal-subtree");
  assert.ok(subtree.textContent.includes("Keep writes atomic"));
  assert.ok(q("goal-row-cas") && q("goal-row-val"));
  assert.equal(q("goal-row-ui"), null, "sibling goals stay out");

  await act(async () => fireEvent.click(q("goal-status-val")));
  assert.deepEqual(calls.at(-1), {
    op: "update",
    id: "val",
    status: "in_progress",
  });
  // The linked goal's own status toggles from the summary line.
  await act(async () => fireEvent.click(q("goal-status-srv")));
  assert.deepEqual(calls.at(-1), {
    op: "update",
    id: "srv",
    status: "in_progress",
  });

  await act(async () => fireEvent.click(q("goal-add-srv")));
  const title = container.querySelector("[data-testid='goal-editor-title']");
  await act(async () =>
    fireEvent.change(title, { target: { value: "Retry on conflict" } }),
  );
  await act(async () =>
    fireEvent.submit(container.querySelector("[data-testid='goal-editor']")),
  );
  const added = calls.at(-1);
  assert.equal(added.op, "add");
  assert.equal(added.parent, "srv");
  assert.equal(added.title, "Retry on conflict");
  await unmount();
});

test("an unlinked thread keeps the link picker", async () => {
  const { element } = view({ threadRootId: "cd".repeat(32) });
  const { q, unmount } = await mount(element);
  assert.equal(q("thread-goal-panel"), null);
  assert.ok(q("thread-goal-chip"));
  assert.ok(q("thread-goal-link").textContent.includes("Link this thread"));
  await unmount();
});

test("no goal tree renders nothing", async () => {
  const { element } = view({ tree: { v: 1, nodes: [] } });
  const { container, unmount } = await mount(element);
  assert.equal(container.innerHTML, "");
  await unmount();
});

test("the picker links, unlinks, and adds-and-links in one revision", async () => {
  const calls = [];
  let done = 0;
  const other = "cd".repeat(32);
  const { container, q, unmount } = await mount(
    React.createElement(GoalLinkPicker, {
      apply: async (op) => {
        calls.push(op);
      },
      linkedGoalId: null,
      onDone: () => {
        done += 1;
      },
      threadRootId: other,
      tree,
    }),
  );
  assert.equal(q("thread-goal-unlink"), null);
  await act(async () => fireEvent.click(q("thread-goal-option-ui")));
  assert.deepEqual(calls.at(-1), { op: "link", id: "ui", thread: other });

  await act(async () => fireEvent.click(q("thread-goal-create-under-ui")));
  assert.ok(q("thread-goal-create").textContent.includes("layer 3"));
  const title = container.querySelector("[data-testid='goal-editor-title']");
  await act(async () =>
    fireEvent.change(title, { target: { value: "Thread panel" } }),
  );
  await act(async () =>
    fireEvent.submit(container.querySelector("[data-testid='goal-editor']")),
  );
  const batch = calls.at(-1);
  assert.ok(Array.isArray(batch), "add and link go out as one revision");
  assert.equal(batch.length, 2);
  assert.equal(batch[0].op, "add");
  assert.equal(batch[0].parent, "ui");
  assert.equal(batch[0].title, "Thread panel");
  assert.deepEqual(batch[1], { op: "link", id: batch[0].id, thread: other });
  assert.equal(done, 2);
  await unmount();
});

test("a linked thread's picker can unlink", async () => {
  const calls = [];
  const { q, unmount } = await mount(
    React.createElement(GoalLinkPicker, {
      apply: async (op) => {
        calls.push(op);
      },
      linkedGoalId: "srv",
      onDone: () => {},
      threadRootId: THREAD,
      tree,
    }),
  );
  await act(async () => fireEvent.click(q("thread-goal-unlink")));
  assert.deepEqual(calls, [{ op: "unlink", thread: THREAD }]);
  await unmount();
});
