import assert from "node:assert/strict";
import test from "node:test";

import { relayOrigin, watchTargets } from "./targets.ts";

const A = "a".repeat(64);
const B = "B".repeat(64);
const account = (origin, pubkey) => ({
  origin,
  pubkey,
  displayName: null,
  buzzName: null,
  email: null,
});

test("relayOrigin matches the native origin_for", () => {
  assert.equal(
    relayOrigin("wss://Relay.Example:8443/"),
    "https://relay.example:8443",
  );
  assert.equal(relayOrigin("ws://localhost:3000"), "http://localhost:3000");
  assert.equal(relayOrigin("https://not-a-ws"), null);
});

test("watchTargets skips the open account and servers with no community", () => {
  const communities = [
    { id: "1", name: "one", relayUrl: "wss://one.example", addedAt: "" },
    { id: "2", name: "two", relayUrl: "wss://two.example/", addedAt: "" },
  ];
  const targets = watchTargets(
    [
      account("https://one.example", A),
      account("https://one.example", B),
      account("https://two.example", A),
      account("https://gone.example", B),
    ],
    communities,
    { origin: "https://one.example", pubkey: A },
  );
  assert.deepEqual(
    targets.map((t) => [t.key, t.relayUrl]),
    [
      [`https://one.example|${B.toLowerCase()}`, "wss://one.example"],
      [`https://two.example|${A}`, "wss://two.example/"],
    ],
  );
});
