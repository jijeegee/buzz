import assert from "node:assert/strict";
import { test } from "node:test";

import {
  applyUsersBatchAnswer,
  createUsersBatchLoader,
  knownProfiles,
  newerHeldProfile,
  resetUsersBatchLoaders,
} from "./usersBatchLoader.ts";

const profile = (name) => ({
  displayName: name,
  name,
  avatarUrl: null,
  nip05Handle: null,
  ownerPubkey: null,
});

test("community reset rejects queued reads before they reach the new relay", async () => {
  let fire;
  let reads = 0;
  const load = createUsersBatchLoader(
    async () => {
      reads += 1;
      return { profiles: {}, missing: [] };
    },
    {
      setTimeout(fn) {
        fire = fn;
        return 1;
      },
    },
  );
  const pending = load(["a"]);
  const rejected = assert.rejects(pending, /invalidated/);
  resetUsersBatchLoaders();
  fire();
  await rejected;
  assert.equal(reads, 0);
  await assert.rejects(load(["a"]), /invalidated/);
});

test("invalidating an in-flight loader rejects old data while a replacement reads fresh data", async () => {
  let fire;
  let finish;
  let started;
  const begun = new Promise((resolve) => {
    started = resolve;
  });
  const load = createUsersBatchLoader(
    () => {
      started();
      return new Promise((resolve) => {
        finish = resolve;
      });
    },
    {
      setTimeout(fn) {
        fire = fn;
        return 1;
      },
    },
  );
  const pending = load(["a"]);
  const rejected = assert.rejects(pending, /invalidated/);
  fire();
  await begun;
  load.dispose();
  const replacement = createUsersBatchLoader(
    async () => ({
      profiles: { a: profile("After save") },
      missing: [],
    }),
    {
      setTimeout(fn) {
        fire = fn;
        return 1;
      },
    },
  );
  const fresh = replacement(["a"]);
  fire();
  finish({ profiles: { a: profile("Before save") }, missing: [] });
  await rejected;
  assert.equal((await fresh).profiles.a.displayName, "After save");
});

test("community reset rejects responses already in flight", async () => {
  let fire;
  let finish;
  let started;
  const begun = new Promise((resolve) => {
    started = resolve;
  });
  const load = createUsersBatchLoader(
    () => {
      started();
      return new Promise((resolve) => {
        finish = resolve;
      });
    },
    {
      setTimeout(fn) {
        fire = fn;
        return 1;
      },
    },
  );
  const pending = load(["a"]);
  const rejected = assert.rejects(pending, /invalidated/);
  fire();
  await begun;
  resetUsersBatchLoaders();
  finish({ profiles: { a: profile("Old community") }, missing: [] });
  await rejected;
});

test("rows asking together share one relay read, without repeats", async () => {
  const calls = [];
  let fire = () => {};
  const load = createUsersBatchLoader(
    async (pubkeys) => {
      calls.push([...pubkeys].sort());
      return {
        profiles: Object.fromEntries(
          pubkeys.filter((p) => p !== "gone").map((p) => [p, profile(p)]),
        ),
        missing: pubkeys.filter((p) => p === "gone"),
      };
    },
    {
      setTimeout(fn) {
        fire = fn;
        return 1;
      },
    },
  );
  const first = load(["a", "b"]);
  const second = load(["b", "c", "gone"]);
  fire();
  const [one, two] = await Promise.all([first, second]);
  assert.deepEqual(calls, [["a", "b", "c", "gone"]]);
  assert.deepEqual(Object.keys(one.profiles).sort(), ["a", "b"]);
  assert.deepEqual(Object.keys(two.profiles).sort(), ["b", "c"]);
  assert.deepEqual(two.missing, ["gone"]);
});

const manualTimers = () => {
  const timers = { fire: () => {} };
  timers.setTimeout = (fn) => {
    timers.fire = fn;
    return 1;
  };
  return timers;
};

const ID_A = "a".repeat(64);
const ID_B = "b".repeat(64);
const heldProfile = (name, createdAt, verifiedAt, eventId = ID_A) => ({
  ...profile(name),
  createdAt,
  verifiedAt,
  eventId,
});

test("held profiles are asked only for changes and come back unchanged", async () => {
  const calls = [];
  const timers = manualTimers();
  const load = createUsersBatchLoader(async (pubkeys, known) => {
    calls.push({ pubkeys, known });
    return {
      profiles: { changed: { ...profile("New"), createdAt: 300 } },
      missing: ["gone"],
      unchanged: ["same"],
    };
  }, timers);
  const held = {
    same: heldProfile("Same", 100, 5_000),
    changed: heldProfile("Old", 200, 5_000),
  };

  const pending = load(["same", "changed", "gone"], held);
  timers.fire();
  const answer = await pending;

  assert.deepEqual(calls, [
    {
      pubkeys: ["same", "changed", "gone"],
      known: {
        same: { createdAt: 100, verifiedAt: 5_000, eventId: ID_A },
        changed: { createdAt: 200, verifiedAt: 5_000, eventId: ID_A },
      },
    },
  ]);
  assert.deepEqual(Object.keys(answer.profiles), ["changed"]);
  assert.deepEqual(answer.unchanged, ["same"]);
  assert.equal(answer.kept.same, held.same);
  assert.deepEqual(answer.missing, ["gone"]);
});

test("a pubkey gathered from an asker holding nothing is asked in full", async () => {
  const calls = [];
  const timers = manualTimers();
  const load = createUsersBatchLoader(async (_pubkeys, known) => {
    calls.push(known);
    return { profiles: {}, missing: [], unchanged: [] };
  }, timers);

  const first = load(["a", "b"], {
    a: heldProfile("A", 100, 900),
    b: heldProfile("B", 300, 700),
  });
  const second = load(["a", "b"], { b: heldProfile("B", 300, 800, ID_B) });
  timers.fire();
  await Promise.all([first, second]);

  // b: the profile the relay would replace (same second, higher id), with
  // the earliest check, so the answer covers both askers.
  assert.deepEqual(calls, [
    { b: { createdAt: 300, verifiedAt: 700, eventId: ID_B } },
  ]);
});

test("an asker holding nothing that joins a read in flight gets the profile another asker held", async () => {
  const timers = manualTimers();
  let finish;
  let started;
  const begun = new Promise((resolve) => {
    started = resolve;
  });
  const load = createUsersBatchLoader(() => {
    started();
    return new Promise((resolve) => {
      finish = resolve;
    });
  }, timers);
  const held = { a: heldProfile("A", 100, 900) };

  const holder = load(["a"], held);
  timers.fire();
  await begun;
  const empty = load(["a"]);
  finish({ profiles: {}, missing: [], unchanged: ["a"] });
  const [, answer] = await Promise.all([holder, empty]);

  assert.deepEqual(answer.missing, []);
  const applied = applyUsersBatchAnswer(["a"], {}, answer, 5_000);
  assert.deepEqual(applied.profiles.a, { ...held.a, verifiedAt: 5_000 });
  assert.deepEqual(applied.missing, []);
});

test("only profiles with created_at, verified_at and event id are known; earlier saves are read in full", () => {
  assert.deepEqual(
    knownProfiles({
      fresh: heldProfile("Fresh", 100, 900),
      noId: { ...profile("NoId"), createdAt: 100, verifiedAt: 900 },
      unchecked: { ...profile("Unchecked"), createdAt: 100, eventId: ID_A },
      legacy: profile("Legacy"),
    }),
    { fresh: { createdAt: 100, verifiedAt: 900, eventId: ID_A } },
  );
  const legacy = profile("Legacy");
  const fresh = heldProfile("Fresh", 100, 900, ID_B);
  assert.equal(newerHeldProfile(legacy, fresh), fresh);
  assert.equal(newerHeldProfile(fresh, legacy), fresh);
  const newer = heldProfile("Newer", 200, 800);
  assert.equal(newerHeldProfile(fresh, newer), newer);
  // Same second: the lower id is the one the relay keeps.
  const lowerId = heldProfile("Lower", 100, 800, ID_A);
  assert.equal(newerHeldProfile(fresh, lowerId), lowerId);
  const rechecked = { ...fresh, verifiedAt: 950 };
  assert.equal(newerHeldProfile(fresh, rechecked), rechecked);
  assert.equal(newerHeldProfile(undefined, legacy), legacy);
});

test("an unchanged held profile is kept and never missing; a newer one replaces it; both take the pre-request check time", () => {
  const held = {
    same: { ...heldProfile("Same", 100, 900), ownerPubkey: "owner" },
    changed: heldProfile("Old", 200, 900),
  };
  const answer = applyUsersBatchAnswer(
    ["same", "changed", "gone"],
    held,
    {
      profiles: { changed: { ...profile("New"), createdAt: 300 } },
      missing: ["gone"],
      unchanged: ["same"],
      kept: { same: held.same },
    },
    5_000,
  );

  assert.deepEqual(answer.profiles.same, { ...held.same, verifiedAt: 5_000 });
  assert.equal(answer.profiles.changed.displayName, "New");
  assert.equal(answer.profiles.changed.createdAt, 300);
  assert.equal(answer.profiles.changed.verifiedAt, 5_000);
  assert.deepEqual(answer.missing, ["gone"]);
});
