import assert from "node:assert/strict";
import test from "node:test";

import { applyContactChange, editContactList } from "./contactListEdit.ts";

test("adding keeps everyone already there, with their hints", () => {
  const contacts = [{ pubkey: "aa", relayUrl: "wss://x", petname: "Al" }];
  assert.deepEqual(
    applyContactChange(contacts, { type: "add", pubkeys: ["BB", "aa"] }),
    [{ pubkey: "aa", relayUrl: "wss://x", petname: "Al" }, { pubkey: "bb" }],
  );
});

test("a change that changes nothing skips the write", () => {
  const contacts = [{ pubkey: "aa" }];
  assert.equal(
    applyContactChange(contacts, { type: "add", pubkeys: ["AA"] }),
    null,
  );
  assert.equal(
    applyContactChange(contacts, { type: "remove", pubkey: "zz" }),
    null,
  );
});

test("removing drops only that person", () => {
  const contacts = [{ pubkey: "aa" }, { pubkey: "bb" }];
  assert.deepEqual(
    applyContactChange(contacts, { type: "remove", pubkey: "AA" }),
    [{ pubkey: "bb" }],
  );
});

function fakeServer(initial) {
  let contacts = initial;
  let clock = 10_000;
  const writes = [];
  const sleeps = [];
  return {
    writes,
    sleeps,
    contacts: () => contacts,
    io: {
      read: async () => ({ contacts }),
      write: async (next) => {
        writes.push({ at: Math.floor(clock / 1000), next });
        contacts = next;
      },
      now: () => clock,
      sleep: async (ms) => {
        sleeps.push(ms);
        clock += ms;
      },
    },
  };
}

test("back-to-back changes re-read the latest list and never collide", async () => {
  const server = fakeServer([{ pubkey: "aa" }]);
  const results = await Promise.all([
    editContactList("me1", { type: "add", pubkeys: ["bb"] }, server.io),
    editContactList("me1", { type: "add", pubkeys: ["cc"] }, server.io),
    editContactList("me1", { type: "remove", pubkey: "aa" }, server.io),
  ]);
  assert.deepEqual(results, [true, true, true]);
  assert.deepEqual(
    server.contacts().map((c) => c.pubkey),
    ["bb", "cc"],
  );
  const seconds = server.writes.map((w) => w.at);
  assert.equal(new Set(seconds).size, seconds.length, "one write per second");
  assert.equal(server.sleeps.length, 2);
});

test("a failed change does not block the next one", async () => {
  const server = fakeServer([]);
  const failing = {
    ...server.io,
    write: async () => {
      throw new Error("offline");
    },
  };
  await assert.rejects(
    editContactList("me2", { type: "add", pubkeys: ["aa"] }, failing),
    /offline/,
  );
  assert.equal(
    await editContactList("me2", { type: "add", pubkeys: ["aa"] }, server.io),
    true,
  );
  assert.deepEqual(server.contacts(), [{ pubkey: "aa" }]);
});

test("an edit writes back the list's content and other tags as they were", async () => {
  const kept = [];
  const relays = '{"wss://relay.example":{"read":true}}';
  const io = {
    read: async () => ({
      contacts: [{ pubkey: "aa", relayUrl: "wss://hint", petname: "Al" }],
      content: relays,
      otherTags: [["t", "kuzz"]],
    }),
    write: async (next, keep) => kept.push({ next, keep }),
  };
  await editContactList("me3", { type: "add", pubkeys: ["bb"] }, io);
  assert.deepEqual(kept, [
    {
      next: [
        { pubkey: "aa", relayUrl: "wss://hint", petname: "Al" },
        { pubkey: "bb" },
      ],
      keep: { content: relays, otherTags: [["t", "kuzz"]] },
    },
  ]);
});
