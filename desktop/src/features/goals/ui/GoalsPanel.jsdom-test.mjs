import assert from "node:assert/strict";
import { afterEach, mock, test } from "node:test";

// The goals panel over the relay's read shape: the active outline, the quiet
// and done lists read when opened, the notice about many top goals, and the
// one edit behind "Drop all".

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent } from "@testing-library/react";
import React, { act } from "react";
import { createRoot } from "react-dom/client";

import { roomLive } from "../../../shared/api/relayRoomLive.ts";
import { GoalsPanel } from "./GoalsPanel.tsx";

const CHANNEL = "c1";

const row = (id, parent, extra = {}) => ({
  id,
  parent,
  title: `Goal ${id}`,
  note: "",
  status: "open",
  order: 0,
  assignees: [],
  links: [],
  updated_by: null,
  updated_at: 0,
  matched: true,
  inactive: false,
  is_thread: false,
  progress: { done: 0, total: 0 },
  open_children: 0,
  ...extra,
});

const page = (rows, counts) => ({
  counts: { active: 0, inactive: 0, done: 0, ...counts },
  rows,
  threads: [],
  next: null,
});

let cleanup = null;

afterEach(async () => {
  await cleanup?.();
  cleanup = null;
  mock.restoreAll();
  delete window.__TAURI_INTERNALS__;
});

const settle = () =>
  act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 0));
  });

async function mount(reads) {
  const calls = [];
  window.__TAURI_INTERNALS__ = {
    invoke: async (command, args) => {
      calls.push({ command, args });
      if (command === "get_goals") return reads[args.view];
      if (command === "apply_goal_ops") return { event_id: "e" };
      throw new Error(`unexpected ${command}`);
    },
  };
  mock.method(roomLive, "listen", () => () => {});
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  await act(async () => {
    root.render(
      React.createElement(
        QueryClientProvider,
        { client },
        React.createElement(GoalsPanel, { channelId: CHANNEL }),
      ),
    );
  });
  await settle();
  cleanup = async () => {
    await act(async () => root.unmount());
    host.remove();
  };
  return {
    calls,
    q: (testId) => host.querySelector(`[data-testid='${testId}']`),
    click: async (element) => {
      await act(async () => fireEvent.click(element));
      await settle();
    },
    reads: (view) =>
      calls.filter((c) => c.command === "get_goals" && c.args.view === view)
        .length,
  };
}

test("a quiet list reads when opened, shows the path, and drops as one edit", async () => {
  const counts = { active: 1, inactive: 1, done: 0 };
  const panel = await mount({
    active: page([row("root", null)], counts),
    inactive: page(
      [
        row("root", null, { matched: false }),
        row("plan", "root", { matched: false }),
        row("old", "plan", { inactive: true }),
      ],
      counts,
    ),
  });
  assert.equal(panel.q("goals-section-done"), null);
  assert.equal(panel.reads("inactive"), 0);

  await panel.click(panel.q("goals-section-toggle-inactive"));
  assert.equal(panel.reads("inactive"), 1);
  const section = panel.q("goals-section-inactive");
  assert.ok(panel.q("goal-row-old"));
  assert.equal(panel.q("goal-row-plan"), null);
  assert.match(section.textContent, /Goal plan/);

  await panel.click(panel.q("goals-drop-quiet"));
  await panel.click(panel.q("goals-drop-quiet-confirm"));
  const edit = panel.calls.find((c) => c.command === "apply_goal_ops");
  assert.deepEqual(edit.args.ops, [{ op: "drop_inactive" }]);
});

test("many goals under layer 1 get a notice, and nothing stops adding", async () => {
  const many = await mount({
    active: page([row("root", null, { open_children: 8 })], { active: 9 }),
  });
  assert.match(many.q("goals-top-notice").textContent, /8/);
  assert.ok(many.q("goal-add-root"));
  await cleanup();
  cleanup = null;

  const few = await mount({
    active: page([row("root", null, { open_children: 7 })], { active: 8 }),
  });
  assert.equal(few.q("goals-top-notice"), null);
});

test("removing sends the whole branch: the list cannot see what is below", async () => {
  const panel = await mount({
    active: page([row("root", null), row("a", "root")], { active: 2 }),
  });
  await panel.click(panel.q("goal-remove-a"));
  await panel.click(panel.q("goal-remove-confirm-a"));
  const edit = panel.calls.find((c) => c.command === "apply_goal_ops");
  assert.deepEqual(edit.args.ops, [{ op: "remove", id: "a", recursive: true }]);
});

test("the room has no goals yet: the layer 1 form", async () => {
  const panel = await mount({ active: page([], {}) });
  assert.ok(panel.q("goals-empty"));
});

test("finishing a thread goal with tasks asks first", async () => {
  const panel = await mount({
    active: page(
      [
        row("root", null),
        row("t", "root", {
          status: "in_progress",
          is_thread: true,
          progress: { done: 0, total: 2 },
          open_children: 2,
        }),
      ],
      { active: 2 },
    ),
  });
  await panel.click(panel.q("goal-status-t"));
  const edits = () => panel.calls.filter((c) => c.command === "apply_goal_ops");
  assert.equal(edits().length, 0);
  await panel.click(
    document.querySelector("[data-testid='goal-finish-confirm-t']"),
  );
  assert.deepEqual(edits()[0].args.ops, [
    { op: "update", id: "t", status: "done" },
  ]);
});

test("a thread goal with nothing listed below it can still take a sub-goal", async () => {
  const panel = await mount({
    active: page([row("root", null), row("t", "root", { is_thread: true })], {
      active: 2,
    }),
  });
  assert.equal(panel.q("goal-add-t"), null);
  const row_ = panel.q("goal-row-t");
  await panel.click(row_.querySelector("button"));
  assert.ok(panel.q("goal-add-t"));
});
