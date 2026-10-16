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
  assert.ok(
    subtree.contains(q("goal-note-toggle-srv")),
    "notes wait behind a button",
  );
  assert.ok(q("goal-row-cas") && q("goal-row-val"));
  // The whole tree shows; only the linked goal's branch is editable.
  const access = (id) => q(`goal-row-${id}`).dataset.goalAccess;
  assert.equal(access("srv"), "focus");
  assert.equal(access("val"), "edit");
  assert.equal(access("root"), "path");
  assert.equal(access("ui"), "muted", "other goals show faded");
  for (const id of ["root", "ui"]) {
    assert.equal(q(`goal-status-${id}`).tagName, "SPAN", `${id} read-only`);
    assert.equal(q(`goal-add-${id}`), null);
    assert.equal(q(`goal-remove-${id}`), null);
  }
  assert.ok(q("goal-remove-val"));

  await act(async () => fireEvent.click(q("goal-status-val")));
  assert.deepEqual(calls.at(-1), {
    op: "update",
    id: "val",
    status: "in_progress",
  });
  // The linked goal's own status toggles from the summary line.
  await act(async () =>
    fireEvent.click(
      q("thread-goal-panel").querySelector("[data-testid='goal-status-srv']"),
    ),
  );
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

test("the header popover opens with the whole tree showing", async () => {
  const linked = await mount(view({ defaultExpanded: true }).element);
  assert.equal(
    linked.q("thread-goal-toggle").getAttribute("aria-expanded"),
    "true",
  );
  assert.equal(linked.q("goal-row-srv").dataset.goalAccess, "focus");
  assert.equal(linked.q("goal-row-ui").dataset.goalAccess, "muted");
  await linked.unmount();

  // Unlinked: the link picker, and the whole tree faded and read-only.
  const unlinked = await mount(
    view({ defaultExpanded: true, threadRootId: "cd".repeat(32) }).element,
  );
  assert.ok(unlinked.q("thread-goal-link"));
  for (const id of ["root", "srv", "ui"]) {
    assert.equal(unlinked.q(`goal-row-${id}`).dataset.goalAccess, "muted");
    assert.equal(unlinked.q(`goal-add-${id}`), null);
  }
  await unlinked.unmount();
});

test("thread links go to other threads; this thread's link stays put", async () => {
  const OTHER = "ef".repeat(32);
  const opened = [];
  const linkedTree = {
    ...tree,
    nodes: tree.nodes.map((n) =>
      n.id === "ui" ? { ...n, threads: [OTHER] } : n,
    ),
  };
  const { element } = view({
    defaultExpanded: true,
    onOpenLink: (target) => opened.push(target.threadRootId),
    tree: linkedTree,
  });
  const { q, unmount } = await mount(element);
  const here = q(`goal-thread-${THREAD}`);
  assert.equal(here.getAttribute("aria-label"), "This thread");
  assert.equal(here.textContent, "", "links are icons only");
  assert.equal(here.disabled, true);
  const other = q(`goal-thread-${OTHER}`);
  assert.equal(other.disabled, false, "faded goals still navigate");
  await act(async () => fireEvent.click(other));
  assert.deepEqual(opened, [OTHER]);
  await unmount();
});

test("linked messages read Message and jump; missing ones are disabled", async () => {
  const [MAIN, REPLY, GONE, ROOT, LOADING] = ["11", "22", "33", "44", "55"].map(
    (c) => c.repeat(32),
  );
  const opened = [];
  const linkedTree = {
    ...tree,
    nodes: tree.nodes.map((n) =>
      n.id === "ui" ? { ...n, threads: [MAIN, REPLY, GONE, ROOT, LOADING] } : n,
    ),
  };
  const { element } = view({
    defaultExpanded: true,
    linkKinds: new Map([
      [MAIN, { kind: "message" }],
      [REPLY, { kind: "reply", threadRootId: ROOT }],
      [GONE, { kind: "missing" }],
      [ROOT, { kind: "thread" }],
    ]),
    onOpenLink: (target) => opened.push(target),
    tree: linkedTree,
  });
  const { q, unmount } = await mount(element);
  const link = (id) => q(`goal-thread-${id}`);
  assert.deepEqual(
    [MAIN, REPLY, GONE, ROOT, LOADING].map((id) =>
      link(id).getAttribute("aria-label"),
    ),
    [
      "Go to message",
      "Go to message",
      "Go to message",
      "Open thread",
      "Open thread",
    ],
  );
  assert.equal(link(GONE).disabled, true);
  for (const id of [MAIN, REPLY, ROOT]) {
    await act(async () => fireEvent.click(link(id)));
  }
  assert.deepEqual(opened, [
    { kind: "message", messageId: MAIN, threadRootId: null },
    { kind: "message", messageId: REPLY, threadRootId: ROOT },
    { kind: "thread", threadRootId: ROOT },
  ]);
  await unmount();
});

test("a goal's note stays hidden until its note button is pressed", async () => {
  const { element } = view({ defaultExpanded: true, tree });
  const { q, unmount } = await mount(element);
  const row = q("goal-row-srv");
  assert.equal(row.textContent.includes("Keep writes atomic"), false);
  const toggle = q("goal-note-toggle-srv");
  assert.equal(toggle.getAttribute("aria-expanded"), "false");
  await act(async () => fireEvent.click(toggle));
  assert.equal(row.textContent.includes("Keep writes atomic"), true);
  await act(async () => fireEvent.click(toggle));
  assert.equal(row.textContent.includes("Keep writes atomic"), false);
  await unmount();
});

test("a message's picker offers only the goals it may link", async () => {
  const { messageGoalLinkScope } = await import("../goalTree.ts");
  const MESSAGE = "99".repeat(32);
  const picker = async (threadRootId) => {
    const calls = [];
    const mounted = await mount(
      React.createElement(GoalLinkPicker, {
        apply: async (op) => calls.push(op),
        linkedGoalId: null,
        onDone: () => {},
        scope: messageGoalLinkScope(tree, threadRootId),
        subject: "message",
        threadRootId: MESSAGE,
        tree,
      }),
    );
    return { calls, ...mounted };
  };

  // Main timeline: layer 2 and below; a new layer 2 goal may go under L1.
  const main = await picker(null);
  assert.equal(main.q("thread-goal-option-root").disabled, true);
  assert.ok(main.q("thread-goal-create-under-root"));
  await act(async () => fireEvent.click(main.q("thread-goal-option-ui")));
  assert.deepEqual(main.calls, [{ op: "link", id: "ui", thread: MESSAGE }]);
  await main.unmount();

  // In a linked thread: only its goal and sub-goals.
  const linked = await picker(THREAD);
  for (const id of ["srv", "cas", "val"]) {
    assert.equal(linked.q(`thread-goal-option-${id}`).disabled, false);
  }
  for (const id of ["root", "ui"]) {
    assert.equal(linked.q(`thread-goal-option-${id}`), null);
  }
  await linked.unmount();

  // In a thread without a goal: any goal, as the thread's own picker.
  const unlinked = await picker("cd".repeat(32));
  assert.equal(unlinked.q("thread-goal-option-root").disabled, false);
  await unlinked.unmount();
});
