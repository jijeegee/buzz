import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

// Smart routing reaches agents only through the composer's existing
// agent-address path: the pick is appended to `addressedAgentPubkeys` and
// named in `autoRoutedAgentPubkeys`, which `useMentionSendFlow` turns into
// `p` tags plus `["mention", pk, "auto-route"]`. These pins keep the send seam from silently dropping
// the pick or moving it out of the bounded wait. (`useAutoAssign` itself is
// exercised through the real hook in `useAutoAssign.jsdom-test.mjs`.)

async function source(relativePath) {
  return readFile(new URL(relativePath, import.meta.url), "utf8");
}

test("the send awaits the Smart routing pick and addresses it like the tray", async () => {
  const composer = await source("./MessageComposer.tsx");
  const send = composer.slice(composer.indexOf("// Normal send"));
  const resolveAt = send.indexOf("await autoAssign.resolveForSend(trimmed)");
  const flowAt = send.indexOf(
    "await mentionSendFlow.sendMessageWithMentionFlow(",
  );
  assert.ok(resolveAt > 0, "the send resolves the pick");
  assert.ok(flowAt > resolveAt, "before handing off to the mention flow");
  assert.match(
    send,
    /addressedAgentPubkeys: \[\s*\.\.\.persistentAudience\.pubkeys,\s*\.\.\.autoAssignedPubkeys,\s*\],\s*autoRoutedAgentPubkeys: autoAssignedPubkeys,/,
  );
  // An edit during the bounded wait abandons the send instead of clearing
  // keystrokes the user typed after pressing Enter.
  assert.match(
    send,
    /if \(getComposerRevision\(\) !== revisionBeforeRouting\) return;/,
  );
});

test("every text change feeds the router; only the notice renders outside edit mode", async () => {
  const composer = await source("./MessageComposer.tsx");
  const onUpdate = composer.slice(
    composer.indexOf("onUpdate: ({"),
    composer.indexOf("const linkEditor"),
  );
  assert.match(onUpdate, /autoAssignTextRef\.current\(text\);/);
  assert.match(
    composer,
    /editTarget == null \? \(\s*<ComposerAutoAssignRow notice=\{autoAssign\.notice\} \/>/,
  );
});

test("the auto-route mention tag is what the pick becomes", async () => {
  const { buildAgentAddressMentionTags } = await import(
    "../lib/agentAddressMention.mjs"
  );
  const pubkey = "aa".repeat(32);
  assert.deepEqual(buildAgentAddressMentionTags([pubkey], [pubkey], [pubkey]), [
    ["mention", pubkey, "auto-route"],
  ]);
});

test("thread replies give the router their root text", async () => {
  const threadPanel = await source("./MessageThreadPanel.tsx");
  assert.match(threadPanel, /rootContent: threadHead\.body,/);
});
