import assert from "node:assert/strict";
import { test } from "node:test";

import { createUsersBatchLoader } from "./usersBatchLoader.ts";

const profile = (name) => ({
  displayName: name,
  name,
  avatarUrl: null,
  nip05Handle: null,
  ownerPubkey: null,
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
