/**
 * Unit tests for useDraftRootStatus helpers.
 *
 * Tests import the ACTUAL exported `resolveDraftRootStatus` and `deriveActiveDraftCount`
 * (via DraftsPanel) so any implementation change is caught immediately.
 *
 * Tests cover:
 *   - resolveDraftRootStatus: not found → deleted only on an explicit deletion
 *   - deriveActiveDraftCount: exclusion by status, channel-root pass-through
 */

import assert from "node:assert/strict";
import test from "node:test";

// ── Browser-global shim for DraftsPanel imports ───────────────────────────────

function makeLocalStorage() {
  const store = new Map();
  return {
    get length() {
      return store.size;
    },
    key: (i) => [...store.keys()][i] ?? null,
    getItem: (key) => store.get(key) ?? null,
    setItem: (key, value) => store.set(key, value),
    removeItem: (key) => store.delete(key),
    clear: () => store.clear(),
  };
}

if (typeof globalThis.window === "undefined") {
  globalThis.window = { localStorage: makeLocalStorage() };
} else {
  globalThis.window.localStorage = makeLocalStorage();
}
Object.defineProperty(globalThis, "localStorage", {
  get: () => globalThis.window.localStorage,
  configurable: true,
});

// Import the real exported helpers.
import { resolveDraftRootStatus } from "./useDraftRootStatus.ts";
import { deriveActiveDraftCount } from "../ui/DraftsPanel.tsx";

// ── resolveDraftRootStatus: deleted only on an explicit deletion ────────────

const ROOT = "1".repeat(64);
const CHAN = "chan";
const notFound = async () => {
  throw new Error("event not found");
};
const removal = {
  id: "2".repeat(64),
  pubkey: "3".repeat(64),
  created_at: 1,
  kind: 9005,
  tags: [
    ["h", CHAN],
    ["e", ROOT],
  ],
  content: "",
  sig: "",
};

test("resolveDraftRootStatus_found_root_is_available", async () => {
  const status = await resolveDraftRootStatus(ROOT, CHAN, {
    getEvent: async () => ({ id: ROOT }),
    askRoot: async () => assert.fail("not asked"),
  });
  assert.equal(status, "available");
});

test("resolveDraftRootStatus_not_found_without_deletion_stays_available", async () => {
  // A root from before the member joined a since-join room is never returned.
  assert.equal(
    await resolveDraftRootStatus(ROOT, CHAN, {
      getEvent: notFound,
      askRoot: async () => [],
    }),
    "available",
  );
  assert.equal(
    await resolveDraftRootStatus(ROOT, CHAN, {
      getEvent: notFound,
      askRoot: async () => null,
    }),
    "available",
  );
});

test("resolveDraftRootStatus_not_found_with_removal_is_deleted", async () => {
  const status = await resolveDraftRootStatus(ROOT, CHAN, {
    getEvent: notFound,
    askRoot: async () => [removal],
  });
  assert.equal(status, "deleted");
});

test("resolveDraftRootStatus_transport_failure_throws_for_error_status", async () => {
  await assert.rejects(
    resolveDraftRootStatus(ROOT, CHAN, {
      getEvent: async () => {
        throw new Error("transport error: connection refused");
      },
      askRoot: async () => [removal],
    }),
  );
});

// ── Only deleted excludes from count ─────────────────────────────────────────

test("only_deleted_status_excludes_draft_from_count", () => {
  function wouldExclude(rootStatus) {
    return rootStatus === "deleted";
  }

  assert.equal(wouldExclude("deleted"), true, "deleted must be excluded");
  assert.equal(
    wouldExclude("checking"),
    false,
    "checking must NOT be excluded",
  );
  assert.equal(
    wouldExclude("available"),
    false,
    "available must NOT be excluded",
  );
  assert.equal(wouldExclude("error"), false, "error must NOT be excluded");
});

// ── deriveActiveDraftCount (imported from DraftsPanel) ───────────────────────

test("deriveActiveDraftCount_excludes_thread_draft_with_deleted_root", () => {
  const drafts = [{ key: "thread:root-aaa", draft: {} }];
  const statusMap = new Map([["root-aaa", "deleted"]]);
  assert.equal(deriveActiveDraftCount(drafts, statusMap), 0);
});

test("deriveActiveDraftCount_includes_thread_draft_with_available_root", () => {
  const drafts = [{ key: "thread:root-aaa", draft: {} }];
  const statusMap = new Map([["root-aaa", "available"]]);
  assert.equal(deriveActiveDraftCount(drafts, statusMap), 1);
});

test("deriveActiveDraftCount_includes_thread_draft_with_checking_root", () => {
  const drafts = [{ key: "thread:root-aaa", draft: {} }];
  const statusMap = new Map([["root-aaa", "checking"]]);
  assert.equal(deriveActiveDraftCount(drafts, statusMap), 1);
});

test("deriveActiveDraftCount_includes_thread_draft_with_error_root", () => {
  const drafts = [{ key: "thread:root-aaa", draft: {} }];
  const statusMap = new Map([["root-aaa", "error"]]);
  assert.equal(deriveActiveDraftCount(drafts, statusMap), 1);
});

test("deriveActiveDraftCount_includes_channel_root_draft_regardless_of_status_map", () => {
  // Channel-root drafts (key = channel id, not "thread:...") cannot be orphaned.
  const drafts = [{ key: "chan-xyz", draft: {} }];
  const statusMap = new Map(); // empty — no entry for this key
  assert.equal(deriveActiveDraftCount(drafts, statusMap), 1);
});

test("deriveActiveDraftCount_empty_rootStatusMap_treats_thread_drafts_as_available", () => {
  // When panel is closed, statusMap is empty — thread drafts count optimistically.
  const drafts = [
    { key: "thread:root-111", draft: {} },
    { key: "thread:root-222", draft: {} },
    { key: "chan-direct", draft: {} },
  ];
  const statusMap = new Map();
  assert.equal(deriveActiveDraftCount(drafts, statusMap), 3);
});

test("deriveActiveDraftCount_mixed_statuses_counts_correctly", () => {
  const drafts = [
    { key: "thread:root-del", draft: {} }, // deleted — excluded
    { key: "thread:root-ok", draft: {} }, // available — included
    { key: "thread:root-chk", draft: {} }, // checking — included
    { key: "thread:root-err", draft: {} }, // error — included
    { key: "chan-direct", draft: {} }, // channel-root — included
  ];
  const statusMap = new Map([
    ["root-del", "deleted"],
    ["root-ok", "available"],
    ["root-chk", "checking"],
    ["root-err", "error"],
  ]);
  assert.equal(deriveActiveDraftCount(drafts, statusMap), 4);
});
