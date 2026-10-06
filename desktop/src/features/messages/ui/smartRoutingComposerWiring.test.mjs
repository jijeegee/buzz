import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

// Smart routing never delays the send: the composer hands the mention flow
// an `onPublished` callback, and the pick reaches the agent afterwards via
// `deliverAutoRoute` (an edit that newly `p`-tags it plus the `auto-route`
// display tag). These pins keep the send seam from awaiting the router again.
// (`useAutoAssign` itself is exercised through the real hook in
// `useAutoAssign.jsdom-test.mjs`.)

async function source(relativePath) {
  return readFile(new URL(relativePath, import.meta.url), "utf8");
}

test("the send never awaits the router; routing starts on publish", async () => {
  const composer = await source("./MessageComposer.tsx");
  const send = composer.slice(composer.indexOf("// Normal send"));
  assert.doesNotMatch(send, /await autoAssign\./);
  assert.match(
    send,
    /const onPublished =\s*autoAssign\.routeAfterSend\(trimmed, mentionSendFlow\.deliverAutoRoute\)/,
  );
  // A routed send posts agent mentions soft; the router judges them.
  assert.match(
    send,
    /softAgentMentions:\s*autoAssign\.routedByAgent \|\| onPublished !== undefined,/,
  );
  const flow = await source("./useMentionSendFlow.ts");
  assert.match(flow, /if \(published\) draft\.onPublished\?\.\(published\);/);
  assert.match(flow, /AUTO_ROUTE_MENTION_MARKER,\s*\]\),/);
  assert.match(flow, /\.\.\.buildSoftMentionTags\(softPubkeys\),/);
});

test("typing never reaches the router", async () => {
  const composer = await source("./MessageComposer.tsx");
  const onUpdate = composer.slice(
    composer.indexOf("onUpdate: ({"),
    composer.indexOf("const linkEditor"),
  );
  assert.doesNotMatch(onUpdate, /autoAssign/);
});

test("thread replies give the router their root text", async () => {
  const threadPanel = await source("./MessageThreadPanel.tsx");
  assert.match(threadPanel, /rootContent: threadHead\.body,/);
});
