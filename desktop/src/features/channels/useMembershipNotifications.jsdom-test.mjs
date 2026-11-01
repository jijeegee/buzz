/**
 * Membership notices listen on the addressed account bundle, then fetch the
 * last 30 seconds once. A notice that arrives live and again in the fetch is
 * handled once; a catch-up gap refetches the channel list.
 *
 * Falsifiability: restoring `limit: 50, since` on the listener makes
 * meScopeOf reject it (first test); applying fetched events without the
 * shared gate handles the overlap twice (second test).
 */
import assert from "node:assert/strict";
import { afterEach, mock, test } from "node:test";

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import React, { act } from "react";
import { createRoot } from "react-dom/client";

import { relayClient } from "../../shared/api/relayClient.ts";
import { meScopeOf } from "../../shared/api/relayMeBundle.ts";
import { meLive } from "../../shared/api/relayMeLive.ts";
import { useMembershipNotifications } from "./useMembershipNotifications.ts";

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

function notice(id, channelId) {
  return {
    id,
    pubkey: "cd".repeat(32),
    kind: 44100,
    created_at: 100,
    tags: [
      ["p", ME],
      ["h", channelId],
    ],
    content: "",
    sig: "",
  };
}

async function mount() {
  const queryClient = new QueryClient();
  const invalidated = [];
  mock.method(queryClient, "invalidateQueries", async (args) => {
    invalidated.push(args.queryKey);
  });
  function Probe() {
    useMembershipNotifications(ME);
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

function rig() {
  const calls = [];
  const live = [];
  let release;
  const fetched = new Promise((resolve) => {
    release = resolve;
  });
  mock.method(meLive, "subscribe", async (filter, onEvent, _onReady, opts) => {
    calls.push("listen");
    live.push({ filter, onEvent, options: opts });
    return async () => {};
  });
  const fetchEvents = mock.method(relayClient, "fetchEvents", () => {
    calls.push("fetch");
    return fetched.then(() => [...(fetchEvents.events ?? [])]);
  });
  return { calls, live, fetchEvents, release };
}

test("the listener rides the addressed bundle, then one short fetch", async () => {
  const r = rig();
  r.release();
  await mount();

  assert.deepEqual(r.calls, ["listen", "fetch"]);
  const { filter } = r.live[0];
  assert.equal(meScopeOf(filter)?.kind, "addressed");
  assert.equal(filter.limit, 0);
  assert.equal("since" in filter, false);
  const fetchFilter = r.fetchEvents.mock.calls[0].arguments[0];
  assert.equal(fetchFilter.limit, 50);
  const now = Math.floor(Date.now() / 1_000);
  assert.ok(fetchFilter.since <= now - 30 && fetchFilter.since >= now - 35);
});

test("a notice live and in the fetch is handled once", async () => {
  const r = rig();
  const invalidated = await mount();
  await act(async () => r.live[0].onEvent(notice("n1", "room-1")));
  r.fetchEvents.events = [notice("n1", "room-1"), notice("n2", "room-2")];
  await act(async () => r.release());

  const details = invalidated.filter((key) => key.at(-1) === "detail");
  assert.deepEqual(details, [
    ["channels", "room-1", "detail"],
    ["channels", "room-2", "detail"],
  ]);
});

test("a catch-up gap refetches the channel list", async () => {
  const r = rig();
  r.release();
  const invalidated = await mount();
  const before = invalidated.length;
  await act(async () => r.live[0].options.onGap());
  assert.equal(invalidated.length, before + 1);
  assert.deepEqual(invalidated.at(-1), ["channels"]);
});
