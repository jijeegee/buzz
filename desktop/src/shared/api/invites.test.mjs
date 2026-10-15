import assert from "node:assert/strict";
import test, { beforeEach } from "node:test";

import { setBuiltInServerForTests } from "@/features/communities/builtInServer.ts";
import {
  acceptJoinPolicy,
  claimInvite,
  getJoinPolicy,
  mintInvite,
} from "./invites.ts";

// The relay these tests talk to is the built-in server.
beforeEach(() => setBuiltInServerForTests("wss://relay.example"));

function withFetch(response, run) {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async (url) => {
    assert.equal(url, "https://relay.example/api/join-policy");
    return response;
  };
  return Promise.resolve(run()).finally(() => {
    globalThis.fetch = originalFetch;
  });
}

test("getJoinPolicy maps relay-hosted Markdown and age requirements", async () => {
  await withFetch(
    new Response(
      JSON.stringify({
        policy: {
          terms_markdown: "# Terms",
          privacy_markdown: "# Privacy",
          age_attestation_required: true,
          version: "policy-v1",
        },
      }),
      { status: 200 },
    ),
    async () => {
      assert.deepEqual(await getJoinPolicy("wss://relay.example", "webview"), {
        termsMarkdown: "# Terms",
        privacyMarkdown: "# Privacy",
        ageAttestationRequired: true,
        version: "policy-v1",
      });
    },
  );
});

test("getJoinPolicy preserves opt-in behavior for unconfigured and older relays", async () => {
  await withFetch(new Response(JSON.stringify({}), { status: 200 }), async () =>
    assert.equal(await getJoinPolicy("wss://relay.example", "webview"), null),
  );
  await withFetch(new Response(null, { status: 404 }), async () =>
    assert.equal(await getJoinPolicy("wss://relay.example", "webview"), null),
  );
});

test("getJoinPolicy fails closed on a policy endpoint error", async () => {
  await withFetch(new Response(null, { status: 503 }), async () =>
    assert.rejects(getJoinPolicy("wss://relay.example", "webview"), /HTTP 503/),
  );
});

test("getJoinPolicy maps the native command response", async () => {
  const previousWindow = globalThis.window;
  globalThis.window = {
    __TAURI_INTERNALS__: {
      invoke(command, args) {
        assert.equal(command, "fetch_join_policy");
        assert.deepEqual(args, { relayUrl: "wss://relay.example" });
        return Promise.resolve({
          terms_markdown: "# Terms",
          privacy_markdown: "# Privacy",
          age_attestation_required: true,
          version: "policy-v1",
        });
      },
    },
  };

  try {
    assert.deepEqual(await getJoinPolicy("wss://relay.example", "native"), {
      termsMarkdown: "# Terms",
      privacyMarkdown: "# Privacy",
      ageAttestationRequired: true,
      version: "policy-v1",
    });
  } finally {
    globalThis.window = previousWindow;
  }
});

// --- mintInvite serialization ---

// The test-loader transpiles TS imports. tauri.ts imports `invoke` from
// @tauri-apps/api/core, which calls `window.__TAURI_INTERNALS__.invoke`.
// We stub that here so getRelayHttpUrl() and signRelayEvent() work in node.

function setupTauriStubs(
  httpBase,
  authEvent = {
    id: "x",
    sig: "y",
    pubkey: "z",
    kind: 27235,
    created_at: 1,
    tags: [],
  },
) {
  const calls = { invokeArgs: [] };
  globalThis.window = globalThis.window ?? {};
  globalThis.window.__TAURI_INTERNALS__ = {
    invoke: async (command, args) => {
      calls.invokeArgs.push({ command, args });
      if (command === "get_relay_http_url") return httpBase;
      if (command === "sign_event") return JSON.stringify(authEvent);
      throw new Error(`Unexpected Tauri command: ${command}`);
    },
  };
  return calls;
}

function teardownTauriStubs() {
  delete globalThis.window.__TAURI_INTERNALS__;
}

test("mintInvite serializes bounded max_uses in the request body", async () => {
  setupTauriStubs("https://relay.example");
  try {
    const originalFetch = globalThis.fetch;
    let capturedBody;
    globalThis.fetch = async (_url, init) => {
      capturedBody = JSON.parse(init.body);
      return new Response(
        JSON.stringify({
          code: "v2.abc123",
          expires_at: 1785100000,
          url: "https://relay.example/invite/v2.abc123",
          max_uses: 10,
          uses_remaining: 10,
        }),
      );
    };
    try {
      const result = await mintInvite({ ttlSecs: 259200, maxUses: 10 });
      assert.equal(capturedBody.ttl_secs, 259200);
      assert.equal(capturedBody.max_uses, 10);
      assert.equal(result.code, "v2.abc123");
      assert.equal(result.maxUses, 10);
      assert.equal(result.usesRemaining, 10);
      assert.equal(result.expiresAt, 1785100000);
      assert.equal(result.url, "https://relay.example/invite/v2.abc123");
    } finally {
      globalThis.fetch = originalFetch;
    }
  } finally {
    teardownTauriStubs();
  }
});

test("mintInvite omits max_uses when null (unlimited)", async () => {
  setupTauriStubs("https://relay.example");
  try {
    const originalFetch = globalThis.fetch;
    let capturedBody;
    globalThis.fetch = async (_url, init) => {
      capturedBody = JSON.parse(init.body);
      return new Response(
        JSON.stringify({
          code: "v2.abc123",
          expires_at: 1785100000,
          url: "https://relay.example/invite/v2.abc123",
          max_uses: null,
          uses_remaining: null,
        }),
      );
    };
    try {
      const result = await mintInvite({ ttlSecs: 259200, maxUses: null });
      assert.equal(capturedBody.ttl_secs, 259200);
      assert.equal(Object.hasOwn(capturedBody, "max_uses"), false);
      assert.equal(result.maxUses, null);
      assert.equal(result.usesRemaining, null);
    } finally {
      globalThis.fetch = originalFetch;
    }
  } finally {
    teardownTauriStubs();
  }
});

test("mintInvite omits max_uses when not provided (unlimited default)", async () => {
  setupTauriStubs("https://relay.example");
  try {
    const originalFetch = globalThis.fetch;
    let capturedBody;
    globalThis.fetch = async (_url, init) => {
      capturedBody = JSON.parse(init.body);
      return new Response(
        JSON.stringify({
          code: "v2.abc123",
          expires_at: 1785100000,
          url: "https://relay.example/invite/v2.abc123",
          max_uses: null,
          uses_remaining: null,
        }),
      );
    };
    try {
      await mintInvite({ ttlSecs: 86400 });
      assert.equal(capturedBody.ttl_secs, 86400);
      assert.equal(Object.hasOwn(capturedBody, "max_uses"), false);
    } finally {
      globalThis.fetch = originalFetch;
    }
  } finally {
    teardownTauriStubs();
  }
});

// --- built-in server guard ---

test("invite requests to another server are refused before any fetch or signing", async () => {
  const calls = setupTauriStubs("https://relay.example");
  const originalFetch = globalThis.fetch;
  const fetched = [];
  globalThis.fetch = async (url) => {
    fetched.push(url);
    return new Response(JSON.stringify({}));
  };
  const refused = /This invite is for a different service\./;
  try {
    for (const relay of [
      "wss://evil.example",
      "ws://relay.example",
      "wss://relay.example:8443",
    ]) {
      await assert.rejects(claimInvite(relay, "code"), refused, relay);
      await assert.rejects(
        acceptJoinPolicy(relay, "code", "v1", true),
        refused,
        relay,
      );
      await assert.rejects(getJoinPolicy(relay, "webview"), refused, relay);
    }
    // Before the built-in server is known (or after reading it failed),
    // nothing is sent, not even to it.
    setBuiltInServerForTests(undefined);
    await assert.rejects(claimInvite("wss://relay.example", "code"), refused);
    await assert.rejects(mintInvite(), refused);
    assert.deepEqual(fetched, []);
    assert.equal(
      calls.invokeArgs.some((call) => call.command === "sign_event"),
      false,
      "no NIP-98 event signed for a refused server",
    );
  } finally {
    globalThis.fetch = originalFetch;
    teardownTauriStubs();
  }
});

test("invite requests to the built-in server go through", async () => {
  setupTauriStubs("https://relay.example");
  const originalFetch = globalThis.fetch;
  const fetched = [];
  globalThis.fetch = async (url) => {
    fetched.push(url);
    return new Response(
      JSON.stringify({
        status: "joined",
        community_id: "c",
        host: "relay.example",
        role: "member",
        receipt: "r",
      }),
    );
  };
  try {
    assert.equal(
      (await claimInvite("wss://relay.example/", "code")).status,
      "joined",
    );
    assert.equal(
      await acceptJoinPolicy("wss://relay.example", "code", "v1", true),
      "r",
    );
    assert.deepEqual(fetched, [
      "https://relay.example/api/invites/claim",
      "https://relay.example/api/invites/accept-policy",
    ]);
  } finally {
    globalThis.fetch = originalFetch;
    teardownTauriStubs();
  }
});
