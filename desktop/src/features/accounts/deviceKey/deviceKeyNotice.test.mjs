import assert from "node:assert/strict";
import test from "node:test";

import { deviceKeyNotice, isDeviceKeyRequired } from "./deviceKeyNotice.ts";

const status = (overrides = {}) => ({
  state: "active",
  reason: null,
  deviceKeyMissing: false,
  ...overrides,
});

test("an active session without a device key asks to sign in again", () => {
  assert.equal(deviceKeyNotice(status({ deviceKeyMissing: true })), "missing");
});

test("a keyless session the relay refused says why it ended", () => {
  for (const reason of ["device_key_required", "me_device_key_required"]) {
    assert.equal(
      deviceKeyNotice(status({ state: "needs_login", reason })),
      "required",
    );
    assert.ok(isDeviceKeyRequired(reason));
  }
});

test("older relays, bots, keyed devices and other endings show nothing", () => {
  assert.equal(deviceKeyNotice(status()), null);
  assert.equal(deviceKeyNotice(status({ deviceKeyMissing: undefined })), null);
  assert.equal(
    deviceKeyNotice(status({ state: "needs_login", reason: "token_expired" })),
    null,
  );
  assert.equal(
    deviceKeyNotice(status({ state: "restoring", deviceKeyMissing: true })),
    null,
  );
  assert.equal(isDeviceKeyRequired(null), false);
});
