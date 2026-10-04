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

test("logout refreshes an expired access token before revoking the cookie", async () => {
  let cookie = true;
  const h = harness((url, init) => {
    if (url.endsWith("/complete")) return loginResponse();
    if (url.endsWith("/refresh"))
      return cookie
        ? json(200, { access: "bzs_fresh", expires_in: 3600 })
        : json(400, {});
    if (init.headers.Authorization === "Bearer bzs_1")
      return json(401, { code: "token_expired" });
    cookie = false;
    return new Response(null, { status: 204 });
  });
  await h.session.completeLogin("code", "verifier");
  await h.session.signOut();
  assert.equal(cookie, false, "logout must actually revoke the cookie");
  await h.session.restore();
  assert.equal(h.session.getSnapshot().status, "signed_out");
});

test("logout rejects unknown or repeated expired 401 and retains retry state", async () => {
  for (const code of ["token_expired", "unknown"]) {
    const h = harness((url) =>
      url.endsWith("/complete")
        ? loginResponse()
        : url.endsWith("/refresh")
          ? json(200, { access: "bzs_new", expires_in: 3600 })
          : json(401, { code }),
    );
    await h.session.completeLogin("code", "verifier");
    await assert.rejects(h.session.signOut(), /Sign-out failed/);
    assert.equal(h.session.getSnapshot().status, "signed_in");
    assert.ok(h.calls.length <= 4);
  }
});

test("a delayed profile body cannot restore account metadata after logout", async () => {
  let release;
  let reading;
  const started = new Promise((r) => {
    reading = r;
  });
  const h = harness((url) => {
    if (url.endsWith("/complete")) return loginResponse();
    if (url.endsWith("/logout")) return new Response(null, { status: 204 });
    return {
      ok: true,
      json: () => {
        reading();
        return new Promise((r) => {
          release = r;
        });
      },
    };
  });
  await h.session.completeLogin("code", "v");
  const pending = h.session.loadProfile();
  await started;
  await h.session.signOut();
  release({ principal_id: "alice", display_name: "Alice" });
  await pending;
  assert.equal(h.session.getSnapshot().principalId, null);
});

test("logout preserves recovery when refresh returns an unknown 401", async () => {
  const h = harness((url) =>
    url.endsWith("/complete")
      ? loginResponse()
      : json(401, {
          code: url.endsWith("/logout")
            ? "token_expired"
            : "upstream_auth_failed",
        }),
  );
  await h.session.completeLogin("code", "v");
  await assert.rejects(h.session.signOut());
  assert.equal(h.session.getSnapshot().status, "signed_in");
});

test("a logout response for a retired account cannot report the new account signed out", async () => {
  let release;
  const h = harness((url) =>
    url.endsWith("/logout")
      ? new Promise((r) => {
          release = r;
        })
      : loginResponse(),
  );
  await h.session.completeLogin("alice", "v");
  const pending = h.session.signOut();
  while (!release) await new Promise((r) => setImmediate(r));
  await h.session.completeLogin("bob", "v");
  release(new Response(null, { status: 204 }));
  await assert.rejects(pending, /account changed/i);
  assert.equal(h.session.getSnapshot().status, "signed_in");
});

test("SignInRequiredError is an Error", () => {
  assert.ok(new SignInRequiredError() instanceof Error);
});
