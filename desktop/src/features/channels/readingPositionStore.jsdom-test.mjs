import assert from "node:assert/strict";
import test from "node:test";

// Reading positions are kept per server and account; a record from before
// that moves once into the first server the account reads on.

const { readingPositionScope, readingPositionStore } = await import(
  "./readingPositionStore.ts"
);

const PUBKEY = "b".repeat(64);
const AT = { messageId: "m1", topPx: 12 };

test("one server's positions are not another's", () => {
  window.localStorage.clear();
  const here = readingPositionScope(PUBKEY, "wss://one.example/");
  const there = readingPositionScope(PUBKEY, "wss://two.example");
  readingPositionStore.set(here, "room", AT);
  assert.deepEqual(readingPositionStore.get(here, "room"), AT);
  assert.equal(readingPositionStore.get(there, "room"), null);
});

test("a socket and a query to one server share a record", () => {
  assert.equal(
    readingPositionScope(PUBKEY, "wss://One.Example:8443/"),
    readingPositionScope(PUBKEY, "https://one.example:8443"),
  );
  assert.equal(readingPositionScope(PUBKEY, undefined), null);
  assert.equal(readingPositionScope(undefined, "wss://one.example"), null);
  assert.equal(readingPositionScope(PUBKEY, "not a url"), null);
});

test("a pre-server record moves once to the first server read", () => {
  window.localStorage.clear();
  window.localStorage.setItem(
    `kuzz-reading-position.v1:${PUBKEY}`,
    JSON.stringify({ room: AT }),
  );
  const here = readingPositionScope(PUBKEY, "wss://one.example");
  const there = readingPositionScope(PUBKEY, "wss://two.example");
  assert.deepEqual(readingPositionStore.get(here, "room"), AT);
  assert.equal(
    window.localStorage.getItem(`kuzz-reading-position.v1:${PUBKEY}`),
    null,
  );
  assert.equal(readingPositionStore.get(there, "room"), null);
});
