/**
 * A route's message target waits for its own fetch: a settled key is not
 * reused in another channel or after the panel closed and reopened.
 */
import assert from "node:assert/strict";
import { test } from "node:test";

import React, { act } from "react";
import { createRoot } from "react-dom/client";

import {
  routeTargetKeyFor,
  useRouteTargetResolution,
} from "./useRouteTargetResolution.ts";

async function mount(initial) {
  const hook = { current: null };
  function Harness({ channelId, targetKey }) {
    hook.current = useRouteTargetResolution(channelId, targetKey);
    return null;
  }
  const root = createRoot(document.createElement("div"));
  const render = (props) =>
    act(async () => root.render(React.createElement(Harness, props)));
  await render(initial);
  return { hook, render, unmount: () => act(async () => root.unmount()) };
}

const KEY = routeTargetKeyFor("m1", "r1");

test("a key settled in one channel does not settle it in another", async () => {
  const { hook, render, unmount } = await mount({ channelId: "A", targetKey: KEY });
  assert.equal(hook.current.isResolving, true);
  await act(async () => hook.current.markResolved("A", KEY));
  assert.equal(hook.current.isResolving, false);
  await render({ channelId: "B", targetKey: KEY });
  assert.equal(hook.current.isResolving, true, "B waits for its own fetch");
  await act(async () => hook.current.markResolved("B", KEY));
  await render({ channelId: "A", targetKey: KEY });
  assert.equal(hook.current.isResolving, true, "back in A waits again");
  await unmount();
});

test("closing and reopening the same target in one room waits again", async () => {
  const { hook, render, unmount } = await mount({ channelId: "A", targetKey: KEY });
  await act(async () => hook.current.markResolved("A", KEY));
  assert.equal(hook.current.isResolving, false);
  await render({ channelId: "A", targetKey: null });
  assert.equal(hook.current.isResolving, false, "no target, nothing pending");
  await render({ channelId: "A", targetKey: KEY });
  assert.equal(hook.current.isResolving, true);
  await unmount();
});
