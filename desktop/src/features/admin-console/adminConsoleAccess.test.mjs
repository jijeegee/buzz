import assert from "node:assert/strict";
import test from "node:test";

import { resolveAdminConsoleAccess } from "./adminConsoleAccess.ts";

const PUBKEY = "a".repeat(64);

function deps({ saved = null, discovered = null, probeState, onProbe } = {}) {
  return {
    getSavedOrigin: async () => saved,
    discoverOrigin: async () => discovered,
    probe: async (origin) => {
      onProbe?.(origin);
      return { state: probeState ?? "nip98Authorized" };
    },
  };
}

test("a saved origin that authorizes grants access", async () => {
  const probed = [];
  const result = await resolveAdminConsoleAccess(
    PUBKEY,
    deps({ saved: "https://admin.example", onProbe: (o) => probed.push(o) }),
  );
  assert.equal(result, true);
  assert.deepEqual(probed, ["https://admin.example"]);
});

test("a same-host advertised origin is probed when nothing is saved", async () => {
  const result = await resolveAdminConsoleAccess(
    PUBKEY,
    deps({ discovered: { origin: "https://relay.example", sameHost: true } }),
  );
  assert.equal(result, true);
});

test("a cross-host advertised origin is never probed", async () => {
  const probed = [];
  const result = await resolveAdminConsoleAccess(
    PUBKEY,
    deps({
      discovered: { origin: "https://elsewhere.example", sameHost: false },
      onProbe: (o) => probed.push(o),
    }),
  );
  assert.equal(result, false);
  assert.deepEqual(probed, []);
});

test("denied, disabled and missing origins hide the section", async () => {
  for (const probeState of ["nip98Denied", "disabled", "notAdminApi"]) {
    assert.equal(
      await resolveAdminConsoleAccess(
        PUBKEY,
        deps({ saved: "https://admin.example", probeState }),
      ),
      false,
    );
  }
  assert.equal(await resolveAdminConsoleAccess(PUBKEY, deps()), false);
});

test("a failing lookup reads as no access", async () => {
  const result = await resolveAdminConsoleAccess(PUBKEY, {
    getSavedOrigin: async () => {
      throw new Error("ipc down");
    },
    discoverOrigin: async () => null,
    probe: async () => ({ state: "nip98Authorized" }),
  });
  assert.equal(result, false);
});
