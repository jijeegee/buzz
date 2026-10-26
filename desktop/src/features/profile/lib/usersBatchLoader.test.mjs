import assert from "node:assert/strict";
import { test } from "node:test";

import {
  createUsersBatchLoader,
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
