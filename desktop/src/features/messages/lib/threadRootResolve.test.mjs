import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import { isRootStandIn, threadRootVerdict } from "./threadRootResolve.ts";

// Shared rules with mobile (test-fixtures/thread-open-root.json).
const fixture = JSON.parse(
  readFileSync(
    new URL("../../../../../test-fixtures/thread-open-root.json", import.meta.url),
    "utf8",
  ),
);

function relayEvent([id, kind, pubkey, target]) {
  const tags = [["h", fixture.channel]];
  if (target) tags.push(["e", target]);
  return { id, kind, pubkey, created_at: 1, content: "", tags, sig: "" };
}

for (const testCase of fixture.cases) {
  test(testCase.name, () => {
    const answer = testCase.answer === null ? null : testCase.answer.map(relayEvent);
    const { verdict, root } = threadRootVerdict(answer, {
      channelId: fixture.channel,
      rootId: fixture.root,
      knownRootAuthor: testCase.known_root_author ?? null,
    });
    assert.equal(verdict, testCase.verdict);
    const rootInAnswer = answer?.some((event) => event.id === fixture.root) ?? false;
    assert.equal(root?.id ?? null, rootInAnswer ? fixture.root : null);
  });
}

test("a root from another channel is not this thread's root", () => {
  const root = { ...relayEvent(["root", 9, "alice", null]), tags: [["h", "elsewhere"]] };
  assert.equal(
    threadRootVerdict([root], { channelId: fixture.channel, rootId: "root" }).verdict,
    "unresolved",
  );
});

test("only placeholder and deleted rows are stand-ins", () => {
  assert.equal(isRootStandIn({ rootPlaceholder: true }), true);
  assert.equal(isRootStandIn({ rootDeleted: true }), true);
  assert.equal(isRootStandIn({}), false);
  assert.equal(isRootStandIn(null), false);
});

test("a reply under a root not loaded yet names that root as its thread", async () => {
  const { resolveReplyRootId } = await import("./threading.ts");
  assert.equal(resolveReplyRootId("root", []), "root");
});
