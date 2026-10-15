import assert from "node:assert/strict";
import test from "node:test";

import { resolveAdminConsoleAccess } from "./adminConsoleAccess.ts";

function deps({ discovered = null, probeState, onProbe } = {}) {
  return {
    discoverOrigin: async () => discovered,
    probe: async (origin) => {
      onProbe?.(origin);
      return { state: probeState ?? "nip98Authorized" };
    },
  };
}

const SAME_HOST = { origin: "https://relay.example", sameHost: true };

test("a same-host advertised origin that authorizes grants access", async () => {
  const probed = [];
  const result = await resolveAdminConsoleAccess(
    deps({ discovered: SAME_HOST, onProbe: (o) => probed.push(o) }),
  );
  assert.equal(result, true);
  assert.deepEqual(probed, ["https://relay.example"]);
});

test("a cross-host advertised origin is never probed", async () => {
  const probed = [];
  const result = await resolveAdminConsoleAccess(
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
        deps({ discovered: SAME_HOST, probeState }),
      ),
      false,
    );
  }
  assert.equal(await resolveAdminConsoleAccess(deps()), false);
});

test("an unreachable server is an error, not a denial", async () => {
  await assert.rejects(
    resolveAdminConsoleAccess(
      deps({ discovered: SAME_HOST, probeState: "networkOrIntercepted" }),
    ),
  );
});

test("a failing discovery lookup is an error, not a denial", async () => {
  await assert.rejects(
    resolveAdminConsoleAccess({
      discoverOrigin: async () => {
        throw new Error("ipc down");
      },
      probe: async () => ({ state: "nip98Authorized" }),
    }),
  );
});
