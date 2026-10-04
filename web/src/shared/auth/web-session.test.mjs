import assert from "node:assert/strict";
import test from "node:test";

import {
  RETRY_DELAYS_MS,
  SignInRequiredError,
  WebSession,
} from "./web-session.ts";

function json(status, body) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

/** A session with a manual clock and captured timers. */
function harness(handler) {
  const calls = [];
  const timers = [];
  const session = new WebSession({
    baseUrl: "https://relay.test/",
    fetch: async (url, init) => {
      calls.push({ url, init });
      return handler(url, init, calls.length);
    },
    now: () => 1_000_000,
    setTimer: (callback, ms) => {
      const timer = { callback, ms, cleared: false };
      timers.push(timer);
      return timer;
    },
    clearTimer: (timer) => {
      timer.cleared = true;
    },
  });
  return { session, calls, timers };
}

const loginResponse = () =>
  json(200, { principal_id: "ab", access: "bzs_1", expires_in: 3600 });

test("concurrent token requests share one cookie refresh", async () => {
  let release;
  const gate = new Promise((resolve) => {
    release = resolve;
  });
  const h = harness(async () => {
    await gate;
    return json(200, { access: "bzs_a", expires_in: 3600 });
  });
  const pending = [
    h.session.getAccessToken(),
    h.session.getAccessToken(),
    h.session.restore(),
  ];
  release();
  assert.deepEqual(await Promise.all(pending), ["bzs_a", "bzs_a", "bzs_a"]);
  assert.equal(h.calls.length, 1);
  const [{ url, init }] = h.calls;
  assert.equal(url, "https://relay.test/auth/refresh");
  assert.equal(init.method, "POST");
  assert.equal(init.credentials, "include");
  assert.equal(init.body, "{}", "the refresh token travels only in the cookie");
  assert.equal(h.session.getSnapshot().status, "signed_in");
  // A cached, fresh token is reused without another request.
  assert.equal(await h.session.getAccessToken(), "bzs_a");
  assert.equal(h.calls.length, 1);
  // Refresh is armed 5 minutes before expiry.
  assert.equal(h.timers.at(-1).ms, 3600_000 - 5 * 60_000);
});

test("terminal refresh failure signs the page out", async () => {
  for (const status of [400, 401]) {
    const h = harness(() => json(status, { code: "refresh_reused" }));
    assert.equal(await h.session.restore(), null);
    assert.equal(h.session.getSnapshot().status, "signed_out");
    // Signed out: token requests do not keep hitting the relay.
    assert.equal(await h.session.getAccessToken(), null);
    assert.equal(h.calls.length, 1);
  }
});

test("transient failures back off and stop after a bounded number", async () => {
  const h = harness(() => json(503, { code: "unavailable" }));
  assert.equal(await h.session.restore(), null);
  assert.equal(h.session.getSnapshot().status, "unavailable");
  for (let i = 0; i < RETRY_DELAYS_MS.length + 3; i += 1) {
    const timer = h.timers.at(-1);
    if (timer.fired) break;
    timer.fired = true;
    timer.callback();
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
  assert.deepEqual(
    h.timers.map((timer) => timer.ms),
    RETRY_DELAYS_MS,
    "one retry per backoff step, then no more timers",
  );
  assert.equal(h.calls.length, RETRY_DELAYS_MS.length + 1);
});

test("a refresh finishing after sign-out cannot resurrect the session", async () => {
  let release;
  const h = harness(async (url) => {
    if (url.endsWith("/auth/oidc/complete")) return loginResponse();
    if (url.endsWith("/auth/logout"))
      return new Response(null, { status: 204 });
    await new Promise((resolve) => {
      release = resolve;
    });
    return json(200, { access: "bzs_stale", expires_in: 3600 });
  });
  await h.session.completeLogin("bzl_code", "verifier");
  const inflight = h.session.refresh();
  await h.session.signOut();
  assert.equal(h.session.getSnapshot().status, "signed_out");
  release();
  await inflight;
  assert.equal(h.session.getSnapshot().status, "signed_out");
  assert.equal(await h.session.getAccessToken(), null);
});

test("sign-out failure keeps the session (the cookie still works)", async () => {
  const h = harness(async (url) =>
    url.endsWith("/auth/oidc/complete") ? loginResponse() : json(500, {}),
  );
  await h.session.completeLogin("bzl_code", "verifier");
  await assert.rejects(() => h.session.signOut(), /Sign-out failed/);
  assert.equal(h.session.getSnapshot().status, "signed_in");
  const logout = h.calls.at(-1);
  assert.equal(logout.url, "https://relay.test/auth/logout");
  assert.equal(logout.init.headers.Authorization, "Bearer bzs_1");
  assert.equal(logout.init.credentials, "include");
});

test("login completion posts the PKCE verifier with cookies enabled", async () => {
  const h = harness(() =>
    json(200, {
      principal_id: "ab".repeat(32),
      access: "bzs_1",
      expires_in: 60,
    }),
  );
  await h.session.completeLogin("bzl_code", "the-verifier");
  const [{ url, init }] = h.calls;
  assert.equal(url, "https://relay.test/auth/oidc/complete");
  assert.equal(init.credentials, "include");
  assert.deepEqual(JSON.parse(init.body), {
    login_code: "bzl_code",
    code_verifier: "the-verifier",
  });
  assert.equal(h.session.getSnapshot().principalId, "ab".repeat(32));
  // A short-lived token still waits at least 30 s before refreshing.
  assert.equal(h.timers.at(-1).ms, 30_000);
});

test("SignInRequiredError is an Error", () => {
  assert.ok(new SignInRequiredError() instanceof Error);
});
