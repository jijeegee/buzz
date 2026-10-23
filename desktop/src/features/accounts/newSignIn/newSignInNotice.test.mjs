import assert from "node:assert/strict";
import test from "node:test";

import { newSignInDeviceId } from "./newSignInHighlight.ts";
import { parseNewSignInNotice } from "./newSignInNotice.ts";

const ME = "a".repeat(64);

function notice(overrides = {}) {
  return {
    id: "e1",
    kind: 24211,
    pubkey: "b".repeat(64),
    created_at: 1,
    tags: [["p", ME]],
    content: "New sign-in: Pixel 9",
    sig: "",
    ...overrides,
  };
}

test("reads the device name from a notice for me", () => {
  assert.deepEqual(parseNewSignInNotice(notice(), ME.toUpperCase()), {
    id: "e1",
    deviceName: "Pixel 9",
  });
});

test("ignores other kinds, other accounts and other text", () => {
  assert.equal(parseNewSignInNotice(notice({ kind: 24210 }), ME), null);
  assert.equal(
    parseNewSignInNotice(notice({ tags: [["p", "c".repeat(64)]] }), ME),
    null,
  );
  assert.equal(parseNewSignInNotice(notice({ content: "hello" }), ME), null);
  assert.equal(
    parseNewSignInNotice(notice({ content: "New sign-in:   " }), ME),
    null,
  );
});

test("strips control characters and caps the name", () => {
  const parsed = parseNewSignInNotice(
    notice({ content: `New sign-in: Lap\u0007top${"x".repeat(100)}` }),
    ME,
  );
  assert.equal(parsed?.deviceName.startsWith("Laptop"), true);
  assert.equal(parsed?.deviceName.length, 64);
});

test("the followed notice marks the newest other device with that name", () => {
  const device = (id, name, current = false) => ({
    id,
    name,
    platform: "android",
    lastSeenAt: null,
    current,
  });
  const devices = [
    device("me", "Galaxy S24", true),
    device("old", "Galaxy S24"),
    device("pixel", "Pixel 8"),
    device("new", "Galaxy S24"),
  ];
  assert.equal(newSignInDeviceId(devices, "Galaxy S24"), "new");
  assert.equal(newSignInDeviceId(devices, "Pixel 8"), "pixel");
  assert.equal(newSignInDeviceId(devices.slice(0, 1), "Galaxy S24"), null);
  assert.equal(newSignInDeviceId(devices, null), null);
});
