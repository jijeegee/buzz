import assert from "node:assert/strict";
import test from "node:test";

import {
  isGoogleBackedAccount,
  needsGoogleProtection,
  onboardingLandingMode,
  showSigningKeySettings,
  signingKeyUiTestSeam,
} from "./signingKeyUi.ts";

function status(overrides = {}) {
  return {
    origin: "https://relay.example",
    supported: true,
    providers: ["google"],
    state: "signed_out",
    principal: null,
    deviceId: null,
    reason: null,
    ...overrides,
  };
}

test("the signing-key seam exists only in E2E mock builds", () => {
  assert.equal(signingKeyUiTestSeam("e2e"), true);
  assert.equal(signingKeyUiTestSeam("production"), false);
  assert.equal(signingKeyUiTestSeam("development"), false);
  // Unit tests run without Vite: the seam stays closed (production behavior).
  assert.equal(signingKeyUiTestSeam(), false);
});

test("users only ever land on Google sign-in, a continue step or a retryable error", () => {
  assert.equal(onboardingLandingMode("loading", "production"), "loading");
  assert.equal(onboardingLandingMode(status(), "production"), "google");
  assert.equal(
    onboardingLandingMode(status({ state: "active" }), "production"),
    "signed-in",
  );
  assert.equal(onboardingLandingMode("error", "production"), "unavailable");
  assert.equal(
    onboardingLandingMode(status({ supported: false }), "production"),
    "unavailable",
  );
  assert.equal(
    onboardingLandingMode(status({ supported: false }), "development"),
    "unavailable",
    "dev builds never fall back to key screens",
  );
});

test("key screens are an E2E-only fallback for mock relays without Google", () => {
  assert.equal(
    onboardingLandingMode(status({ supported: false }), "e2e"),
    "key-fallback",
  );
  assert.equal(onboardingLandingMode("error", "e2e"), "key-fallback");
  assert.equal(
    onboardingLandingMode(status(), "e2e"),
    "google",
    "an E2E relay that offers Google gets the production landing",
  );
});

test("an account is Google-backed once Google holds its sign-in", () => {
  assert.equal(isGoogleBackedAccount(null), false);
  assert.equal(isGoogleBackedAccount(status({ supported: false })), false);
  assert.equal(isGoogleBackedAccount(status()), false);
  assert.equal(isGoogleBackedAccount(status({ keyBackup: true })), true);
  assert.equal(
    isGoogleBackedAccount(status({ legacyTokenAccount: true })),
    true,
  );
  for (const state of ["active", "restoring", "needs_login"]) {
    assert.equal(isGoogleBackedAccount(status({ state })), true, state);
  }
});

test("settings never show key identity or backup outside the E2E seam", () => {
  for (const s of [null, status(), status({ supported: false })]) {
    assert.equal(showSigningKeySettings(s, "production"), false);
  }
  assert.equal(showSigningKeySettings(null, "e2e"), true);
  assert.equal(
    showSigningKeySettings(status({ state: "active", keyBackup: true }), "e2e"),
    false,
  );
});

test("only a local-only account on a Google-capable relay is asked to protect itself", () => {
  assert.equal(
    needsGoogleProtection(status({ keyBackupSupported: true })),
    true,
  );
  assert.equal(needsGoogleProtection(status()), false);
  assert.equal(
    needsGoogleProtection(
      status({ keyBackupSupported: true, legacyTokenAccount: true }),
    ),
    false,
  );
  assert.equal(
    needsGoogleProtection(
      status({ keyBackupSupported: true, state: "active", keyBackup: true }),
    ),
    false,
  );
  assert.equal(needsGoogleProtection(null), false);
});
