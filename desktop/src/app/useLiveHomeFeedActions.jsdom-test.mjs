/**
 * Home feed approvals/reminders and event reminders listen on the account
 * bundle with no history of their own: the home feed and reminders queries
 * hold it. A catch-up that may have missed events (onGap) refreshes them like
 * a live event.
 *
 * Falsifiability: restoring `limit: 50, since` makes meScopeOf reject the
 * filters (first test); dropping the onGap option fails the second.
 */
import assert from "node:assert/strict";
import { afterEach, mock, test } from "node:test";

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import React, { act } from "react";
import { createRoot } from "react-dom/client";

import { relayClient } from "../shared/api/relayClient.ts";
import { meScopeOf } from "../shared/api/relayMeBundle.ts";
import { meLive } from "../shared/api/relayMeLive.ts";
import { useLiveHomeFeedActions } from "./useLiveHomeFeedActions.ts";

const ME = "ab".repeat(32);

let root = null;
let host = null;

afterEach(async () => {
  await act(async () => root?.unmount());
  host?.remove();
  root = null;
  host = null;
  mock.restoreAll();
});

async function mount(onHomeFeedEvent) {
  const queryClient = new QueryClient();
  const invalidated = [];
  mock.method(queryClient, "invalidateQueries", async (args) => {
    invalidated.push(args.queryKey);
  });
  function Probe() {
    useLiveHomeFeedActions(ME, onHomeFeedEvent);
    return null;
  }
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
  await act(async () => {
    root.render(
      React.createElement(
        QueryClientProvider,
        { client: queryClient },
        React.createElement(Probe),
      ),
    );
  });
  return invalidated;
}

test("home feed listeners ride the account bundle and fetch nothing", async () => {
  const subscribed = [];
  mock.method(
    meLive,
    "subscribe",
    async (filter, onEvent, _onReady, options) => {
      subscribed.push({ filter, onEvent, options });
      return async () => {};
    },
  );
  const fetchEvents = mock.method(relayClient, "fetchEvents", async () => []);

  await mount(() => {});

  assert.equal(subscribed.length, 2);
  assert.deepEqual(
    subscribed.map(({ filter }) => meScopeOf(filter)?.kind),
    ["addressed", "authored"],
  );
  for (const { filter } of subscribed) {
    assert.equal(filter.limit, 0);
    assert.equal("since" in filter, false);
  }
  assert.equal(fetchEvents.mock.callCount(), 0);
});

test("a catch-up gap refreshes the home feed and reminders", async () => {
  const subscribed = [];
  mock.method(
    meLive,
    "subscribe",
    async (filter, onEvent, _onReady, options) => {
      subscribed.push({ filter, onEvent, options });
      return async () => {};
    },
  );
  let refreshes = 0;
  const invalidated = await mount(() => {
    refreshes += 1;
  });

  await act(async () => subscribed[0].options.onGap());
  assert.equal(refreshes, 1);
  await act(async () => subscribed[1].options.onGap());
  assert.equal(refreshes, 2);
  assert.equal(invalidated.length, 1, "reminders refetch");
});
