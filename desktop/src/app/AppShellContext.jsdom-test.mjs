/**
 * The AppShell context keeps its identity across AppShell re-renders that
 * change no field, so a live event in another room does not re-render the
 * open room's screen through this context.
 *
 * Falsifiability: passing `value` straight to the provider re-renders the
 * consumer on every provider render.
 */

import assert from "node:assert/strict";
import { test } from "node:test";

import React, { act } from "react";
import { createRoot } from "react-dom/client";

import { AppShellProvider, useAppShell } from "./AppShellContext.tsx";

test("consumers re-render only when a context field changes", async () => {
  const markChannelRead = () => {};
  const unread = new Set(["room-b"]);
  let renders = 0;
  const Consumer = React.memo(function Consumer() {
    useAppShell();
    renders += 1;
    return null;
  });
  const value = (topLevelUnreadChannelIds) => ({
    markChannelRead,
    topLevelUnreadChannelIds,
  });

  const container = document.createElement("div");
  const root = createRoot(container);
  const render = (fields) =>
    act(() => {
      root.render(
        React.createElement(
          AppShellProvider,
          { value: fields },
          React.createElement(Consumer),
        ),
      );
    });

  await render(value(unread));
  assert.equal(renders, 1);
  // A fresh object with the same fields: what AppShell builds on every render.
  await render(value(unread));
  assert.equal(renders, 1);
  await render(value(new Set(["room-b", "room-c"])));
  assert.equal(renders, 2);
  await act(() => root.unmount());
});
