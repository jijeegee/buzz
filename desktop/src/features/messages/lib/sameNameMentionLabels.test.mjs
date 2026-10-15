import assert from "node:assert/strict";
import test from "node:test";

import { snapshotDraftMentionRefs } from "./draftMentionRefs.ts";
import {
  extractMentionPubkeys,
  selectedMentionLabel,
} from "./extractMentionPubkeys.ts";

// Kuzz: people who share a name are told apart by an ordinal fixed at pick
// time ("Scout (2)"), never by a key in the text.
const PERSON = "a".repeat(64);
const AGENT = "b".repeat(64);
const THIRD = "c".repeat(64);
const KEY_IN_TEXT = /[0-9a-f]{64}|npub1/i;

function pick(bindings, name, pubkey) {
  const label = selectedMentionLabel(name, pubkey, bindings);
  bindings.set(label, pubkey);
  return label;
}

test("a single pick keeps the plain name; repeats get fixed ordinals", () => {
  const bindings = new Map();
  assert.equal(pick(bindings, "Scout", PERSON), "Scout");
  assert.equal(pick(bindings, "Scout", PERSON), "Scout");
  assert.equal(pick(bindings, "Scout", AGENT), "Scout (2)");
  assert.equal(pick(bindings, "Scout", AGENT), "Scout (2)");
  assert.equal(pick(bindings, "Scout", THIRD), "Scout (3)");
});

test("the sent body carries no key and tags every recipient", () => {
  const bindings = new Map();
  const labels = [
    pick(bindings, "Unknown user", PERSON),
    pick(bindings, "Unknown user", AGENT),
  ];
  const content = labels.map((label) => `@${label}`).join(" ") + " hi";
  assert.equal(content, "@Unknown user @Unknown user (2) hi");
  assert.doesNotMatch(content, KEY_IN_TEXT);
  assert.deepEqual(
    extractMentionPubkeys({
      text: content,
      selectedMentions: bindings,
      memberCandidates: [],
    }),
    [PERSON, AGENT],
  );
});

test("a person sharing an agent's name is not treated as the agent", () => {
  const bindings = new Map();
  const personLabel = pick(bindings, "Scout", PERSON);
  const agentLabel = pick(bindings, "Scout", AGENT);
  const content = `@${personLabel} @${agentLabel} hi`;
  const refs = snapshotDraftMentionRefs(content, bindings, [agentLabel]);
  const byKey = new Map(refs.map((ref) => [ref.pubkey, ref.isAgent]));
  assert.equal(byKey.get(PERSON), false);
  assert.equal(byKey.get(AGENT), true);
});

test("deleting the first same-name mention never hands it to the second", () => {
  const bindings = new Map();
  pick(bindings, "Scout", PERSON);
  const second = pick(bindings, "Scout", AGENT);
  assert.deepEqual(
    extractMentionPubkeys({
      text: `@${second} hi`,
      selectedMentions: bindings,
      memberCandidates: [],
    }),
    [AGENT],
  );
  assert.deepEqual(
    extractMentionPubkeys({
      text: "@Scout hi",
      selectedMentions: bindings,
      memberCandidates: [],
    }),
    [PERSON],
  );
});
