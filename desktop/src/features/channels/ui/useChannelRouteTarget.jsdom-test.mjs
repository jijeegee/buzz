import assert from "node:assert/strict";
import test from "node:test";

const React = (await import("react")).default;
const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { useChannelRouteTarget } = await import("./useChannelRouteTarget.ts");
const { SEARCH_HIT_PLACEHOLDER_TAG } = await import(
  "../../messages/lib/searchHitPlaceholder.ts"
);

const CHANNEL_ID = "1c7e1c02-87bb-5e88-b2da-5a7a9432d0c9";
const ROOT_ID = "a".repeat(64);
const REPLY_ID = "b".repeat(64);
const CHANNEL = { id: CHANNEL_ID, channelType: "stream" };

function message(id, overrides = {}) {
  return {
    author: "Alice",
    body: id,
    createdAt: 1,
    depth: 0,
    id,
    tags: [["h", CHANNEL_ID]],
    time: "9:00",
    ...overrides,
  };
}

const root = message(ROOT_ID);
const reply = message(REPLY_ID, {
  depth: 1,
  parentId: ROOT_ID,
  rootId: ROOT_ID,
  tags: [
    ["h", CHANNEL_ID],
    ["e", ROOT_ID, "", "reply"],
  ],
});
const replyPlaceholder = message(REPLY_ID, {
  tags: [["h", CHANNEL_ID], [SEARCH_HIT_PLACEHOLDER_TAG]],
});

async function mount(initialProps) {
  const calls = {
    closeAgentSession: 0,
    expandedReplyIds: [],
    openThreadHeadId: [],
    threadScrollTargetId: [],
  };
  let mainTimelineTargetId = null;
  const setters = {
    closeAgentSession: () => {
      calls.closeAgentSession += 1;
    },
    requireThreadEditResolution: () => true,
    setEditTargetId: () => {},
    setExpandedThreadReplyIds: (value) => calls.expandedReplyIds.push(value),
    setOpenThreadHeadId: (value) => calls.openThreadHeadId.push(value),
    setProfilePanelPubkey: () => {},
    setThreadReplyTargetId: () => {},
    setThreadScrollTargetId: (value) => calls.threadScrollTargetId.push(value),
  };
  function Harness(props) {
    mainTimelineTargetId = useChannelRouteTarget({
      activeChannel: CHANNEL,
      activeChannelId: CHANNEL_ID,
      ...setters,
      ...props,
    });
    return null;
  }
  const container = document.createElement("div");
  const reactRoot = createRoot(container);
  const render = async (props) => {
    await act(async () => {
      reactRoot.render(React.createElement(Harness, props));
    });
  };
  await render(initialProps);
  return {
    calls,
    mainTimelineTargetId: () => mainTimelineTargetId,
    render,
    unmount: () => act(async () => reactRoot.unmount()),
  };
}

test("search-hit placeholder waits for the relay copy before routing", async () => {
  const harness = await mount({
    targetMessageId: REPLY_ID,
    timelineMessages: [root, replyPlaceholder],
  });

  // The placeholder has no thread tags; it must not open the reply's own
  // empty panel or lock the route target.
  assert.deepEqual(harness.calls.openThreadHeadId, []);
  assert.equal(harness.calls.closeAgentSession, 0);
  // Nor may it scroll the main timeline, which would clear the route target.
  assert.equal(harness.mainTimelineTargetId(), null);

  await harness.render({
    targetMessageId: REPLY_ID,
    timelineMessages: [root, reply],
  });

  assert.deepEqual(harness.calls.openThreadHeadId, [ROOT_ID]);
  assert.deepEqual(harness.calls.threadScrollTargetId, [REPLY_ID]);
  assert.equal(harness.mainTimelineTargetId(), ROOT_ID);
  await harness.unmount();
});
