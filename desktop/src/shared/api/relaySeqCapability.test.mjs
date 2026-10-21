import assert from "node:assert/strict";
import test from "node:test";

const { probeKuzzSeq } = await import("./relaySeqCapability.ts");

function serve(answer) {
  const calls = [];
  globalThis.fetch = async (url, init) => {
    calls.push({ url, accept: init.headers.Accept });
    if (answer instanceof Error) throw answer;
    return {
      ok: true,
      json: async () => answer,
    };
  };
  return calls;
}

test("NIP-11 kuzz-seq opts in; an old server does not", async () => {
  const calls = serve({ supported_extensions: ["nip-er", "kuzz-seq"] });
  assert.equal(await probeKuzzSeq("wss://new.test"), true);
  assert.deepEqual(calls, [
    { url: "https://new.test/info", accept: "application/nostr+json" },
  ]);
  serve({ supported_extensions: ["nip-er"] });
  assert.equal(await probeKuzzSeq("wss://old.test"), false);
});

test("an unreachable first probe stays off; later probes keep the last answer", async () => {
  serve(new Error("offline"));
  assert.equal(await probeKuzzSeq("wss://down.test"), false);

  serve({ supported_extensions: ["kuzz-seq"] });
  assert.equal(await probeKuzzSeq("wss://flaky.test"), true);
  serve(new Error("offline"));
  assert.equal(await probeKuzzSeq("wss://flaky.test"), true);
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(await probeKuzzSeq("wss://flaky.test"), true);
});

test("a rollback is followed by the next connect", async () => {
  serve({ supported_extensions: ["kuzz-seq"] });
  assert.equal(await probeKuzzSeq("wss://rollback.test"), true);
  serve({ supported_extensions: [] });
  // The cached answer serves this connect while the refresh runs.
  assert.equal(await probeKuzzSeq("wss://rollback.test"), true);
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(await probeKuzzSeq("wss://rollback.test"), false);
});
