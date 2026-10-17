import assert from "node:assert/strict";
import test from "node:test";

import { chatRowPreviewLine } from "./chatRowPreview.ts";

const event = (pubkey, content) => ({
  id: "e1",
  pubkey,
  created_at: 1,
  kind: 9,
  tags: [],
  content,
});

test("a blocked author's latest message never shows its text", () => {
  const blocked = (pk) => pk === "bad";
  assert.equal(
    chatRowPreviewLine(event("bad", "secret text"), blocked),
    "Message from a blocked user",
  );
  assert.equal(chatRowPreviewLine(event("ok", "hello"), blocked), "hello");
  assert.equal(chatRowPreviewLine(event("bad", "hello")), "hello");
});
