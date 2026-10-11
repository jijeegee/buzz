import assert from "node:assert/strict";
import test from "node:test";

import { classifyGoalLinks } from "./goalLinks.ts";

const CHANNEL = "c0ffee00-0000-4000-8000-000000000000";
const [ROOT, LONE, REPLY, OTHER, GONE] = ["aa", "bb", "cc", "dd", "ee"].map(
  (c) => c.repeat(32),
);

const event = (id, tags) => ({ id, tags: [["h", CHANNEL], ...tags] });

test("links classify as thread, message, reply, or missing", async () => {
  const asked = [];
  const kinds = await classifyGoalLinks(
    CHANNEL,
    [ROOT, LONE, REPLY, OTHER, GONE],
    {
      getEventsByIds: async () => [
        event(ROOT, []),
        event(LONE, []),
        event(REPLY, [
          ["e", ROOT, "", "root"],
          ["e", ROOT, "", "reply"],
        ]),
        { id: OTHER, tags: [["h", "another-channel"]] },
      ],
      hasReplies: async (id, channelId) => {
        asked.push([id, channelId]);
        return id === ROOT;
      },
    },
  );
  assert.deepEqual(Object.fromEntries(kinds), {
    [ROOT]: { kind: "thread" },
    [LONE]: { kind: "message" },
    [REPLY]: { kind: "reply", threadRootId: ROOT },
    [OTHER]: { kind: "missing" },
    [GONE]: { kind: "missing" },
  });
  // Only top-level messages need a reply lookup.
  assert.deepEqual(
    asked.sort(),
    [
      [ROOT, CHANNEL],
      [LONE, CHANNEL],
    ].sort(),
  );
});
