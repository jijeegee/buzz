import assert from "node:assert/strict";
import test from "node:test";

import { tokenIdentityChanged } from "./tokenAuth.ts";

test("Google backup login and refresh preserve the signing identity", () => {
  const pubkey = "aa".repeat(32);
  const status = {
    state: "active",
    principal: "bb".repeat(32),
    keyBackup: true,
    signingPubkey: pubkey,
  };
  assert.equal(tokenIdentityChanged(pubkey, status), false);
  assert.equal(
    tokenIdentityChanged(pubkey, { ...status, deviceId: "renewed" }),
    false,
  );
  assert.equal(tokenIdentityChanged("cc".repeat(32), status), true);
  assert.equal(
    tokenIdentityChanged(pubkey, { ...status, state: "needs_login" }),
    false,
  );
});
