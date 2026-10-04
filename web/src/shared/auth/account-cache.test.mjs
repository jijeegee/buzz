import assert from "node:assert/strict";
import test from "node:test";
import { QueryClient } from "@tanstack/react-query";
import { WebSession } from "./web-session.ts";
import * as cache from "./account-cache.ts";

const json = (body) => new Response(JSON.stringify(body));

test("account switch cancels private queries, clears mutations and fences late results", async () => {
  let principal = "alice";
  const session = new WebSession({
    baseUrl: "https://relay.test",
    fetch: async (url) =>
      url.endsWith("/logout")
        ? new Response(null, { status: 204 })
        : json({ access: "access", principal_id: principal, expires_in: 3600 }),
    setTimer: () => 0,
    clearTimer: () => {},
  });
  const client = new QueryClient();
  const scope = cache.bindAccountCache(session, client);
  await session.completeLogin("code", "verifier");
  client.setQueryData(["private"], "alice data");
  client.getMutationCache().build(client, { mutationKey: ["private-write"] });
  let release;
  const pending = client
    .fetchQuery({
      queryKey: ["late"],
      queryFn: () =>
        new Promise((r) => {
          release = r;
        }),
    })
    .catch(() => null);
  const before = scope.getSnapshot();
  await session.signOut();
  assert.notEqual(scope.getSnapshot(), before);
  assert.equal(client.getQueryData(["private"]), undefined);
  assert.equal(client.getMutationCache().getAll().length, 0);
  principal = "bob";
  await session.completeLogin("code", "verifier");
  client.setQueryData(["late"], "bob data");
  release("alice late data");
  await pending;
  assert.equal(client.getQueryData(["late"]), "bob data");
  scope.dispose();
});

test("status blips and learning the principal after restore keep the cache", async () => {
  let now = 0;
  let offline = false;
  const session = new WebSession({
    baseUrl: "https://relay.test",
    now: () => now,
    fetch: async (url) => {
      if (url.endsWith("/auth/me")) return json({ principal_id: "alice" });
      if (offline) throw new Error("offline");
      return json({ access: "access", expires_in: 3600 });
    },
    setTimer: () => 0,
    clearTimer: () => {},
  });
  const client = new QueryClient();
  const scope = cache.bindAccountCache(session, client);
  await session.restore();
  client.setQueryData(["private"], "alice data");
  await session.loadProfile();
  assert.equal(session.getSnapshot().principalId, "alice");

  now += 2 * 3600 * 1000;
  offline = true;
  await session.refresh();
  assert.equal(session.getSnapshot().status, "unavailable");
  offline = false;
  await session.refresh();
  assert.equal(session.getSnapshot().status, "signed_in");

  assert.equal(client.getQueryData(["private"]), "alice data");
  assert.equal(scope.getSnapshot(), 0);
  scope.dispose();
});
