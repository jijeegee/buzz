import assert from "node:assert/strict";
import { afterEach, test } from "node:test";

import { QueryClient } from "@tanstack/react-query";

import {
  afterStartupWindows,
  resetStartupObserverGate,
} from "./startupObserverGate.ts";
import {
  firstOpenWindowSettled,
  openWindowsInFlight,
  resetOpenWindows,
  trackOpenWindow,
} from "../../shared/api/openWindowRequests.ts";

const clients = [];
function newClient() {
  const client = new QueryClient();
  clients.push(client);
  return client;
}
const tick = (ms = 0) => new Promise((resolve) => setTimeout(resolve, ms));

afterEach(() => {
  resetStartupObserverGate();
  resetOpenWindows();
  for (const client of clients.splice(0)) client.clear();
});

test("waits for the relay's channel list and the first room window", async () => {
  const client = newClient();
  // The stored snapshot is not the relay's list.
  client.setQueryData(["channels"], [{ id: "a" }], { updatedAt: 0 });
  let passed = false;
  void afterStartupWindows(client, { room: 1_000, total: 5_000 }).then(
    () => (passed = true),
  );
  let answer;
  const window = trackOpenWindow(new Promise((r) => (answer = r)));
  assert.equal(openWindowsInFlight(), 1);
  client.setQueryData(["channels"], [{ id: "a" }]);
  await tick();
  assert.equal(passed, false, "the room window is still out");
  answer([]);
  await window;
  await tick();
  assert.equal(openWindowsInFlight(), 0);
  assert.equal(passed, true);
  // Later callers pass at once.
  await afterStartupWindows(client);
});

test("with no room open it starts shortly after the channel list", async () => {
  const client = newClient();
  let passed = false;
  void afterStartupWindows(client, { room: 20, total: 5_000 }).then(
    () => (passed = true),
  );
  await tick(40);
  assert.equal(passed, false, "no channel list yet");
  client.setQueryData(["channels"], []);
  await tick(5);
  assert.equal(passed, false);
  await tick(40);
  assert.equal(passed, true);
});

test("never waits longer than the launch cap", async () => {
  const client = newClient();
  await afterStartupWindows(client, { room: 1_000, total: 20 });
});

test("a failed open window still counts as answered", async () => {
  await trackOpenWindow(Promise.reject(new Error("down"))).catch(() => {});
  await firstOpenWindowSettled();
  assert.equal(openWindowsInFlight(), 0);
});

test("a run of the old community never passes the new one", async () => {
  const oldClient = newClient();
  let oldDone = false;
  void afterStartupWindows(oldClient, { room: 10, total: 30 }).then(
    () => (oldDone = true),
  );
  oldClient.setQueryData(["channels"], []);
  // Community switch before the old run's room timer fires.
  resetStartupObserverGate();
  resetOpenWindows();
  await tick();
  assert.equal(oldDone, true, "the old run stops");
  await tick(50);
  const newClientForB = newClient();
  let passed = false;
  void afterStartupWindows(newClientForB, { room: 1_000, total: 5_000 }).then(
    () => (passed = true),
  );
  await tick(20);
  assert.equal(passed, false, "the new community still waits for its own");
});

test("open windows of the old community do not count for the new one", async () => {
  let answer;
  const stale = trackOpenWindow(new Promise((r) => (answer = r)));
  assert.equal(openWindowsInFlight(), 1);
  resetOpenWindows();
  assert.equal(openWindowsInFlight(), 0);
  let settled = false;
  void firstOpenWindowSettled().then(() => (settled = true));
  answer([]);
  await stale;
  await tick();
  assert.equal(settled, false, "a stale settle is ignored");
  assert.equal(openWindowsInFlight(), 0);
  const fresh = trackOpenWindow(Promise.resolve([]));
  assert.equal(openWindowsInFlight(), 1);
  await fresh;
  await tick();
  assert.equal(settled, true);
  assert.equal(openWindowsInFlight(), 0);
});
