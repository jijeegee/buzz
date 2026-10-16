import assert from "node:assert/strict";
import test from "node:test";

const { recordQuickReactionEmoji, setQuickReactionAccount } = await import(
  "./useQuickReactionEmojis.ts"
);

const KEY = "kuzz.quick-reaction-emojis.v1";
const alice = "a".repeat(64);
const bob = "b".repeat(64);

function stored(key) {
  return JSON.parse(localStorage.getItem(key) ?? "[]").map((e) => e.emoji);
}

// Each account on this computer keeps its own recent reactions; the recents
// saved before accounts existed seed every account until it has its own.
test("quick reactions are kept per account, seeded from the community's old recents", () => {
  localStorage.clear();
  localStorage.setItem("kuzz-active-community-id", "c1");
  localStorage.setItem(
    `${KEY}:c1`,
    JSON.stringify([{ emoji: "🔥", count: 3, lastUsedAt: 1 }]),
  );

  setQuickReactionAccount(alice);
  recordQuickReactionEmoji("🚀");
  assert.deepEqual(stored(`${KEY}:c1:${alice}`).sort(), ["🔥", "🚀"].sort());
  assert.deepEqual(stored(`${KEY}:c1`), ["🔥"], "old recents are untouched");

  setQuickReactionAccount(bob);
  recordQuickReactionEmoji("👀");
  assert.deepEqual(stored(`${KEY}:c1:${bob}`).sort(), ["👀", "🔥"].sort());
  assert.equal(
    stored(`${KEY}:c1:${bob}`).includes("🚀"),
    false,
    "another account's recents never leak",
  );
  setQuickReactionAccount(null);
  localStorage.clear();
});
