import assert from "node:assert/strict";
import test from "node:test";

// The sidebar "Working" row opens an agent's activity pane from outside the
// channel and names the thread the agent works in. The pane's back arrow must
// lead to that thread, and the one-shot URL param must be dropped on arrival.

const React = (await import("react")).default;
const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { useChannelAgentSessions } = await import(
  "./useChannelAgentSessions.ts"
);

const AGENT = "a".repeat(64);
const ROOT = "b".repeat(64);

async function mount(props) {
  const calls = { cleared: 0, thread: [], session: [] };
  let latest = null;
  // Stable like the real URL-state setter, so the effect runs once per value.
  const clearAgentSessionReturnThread = () => {
    calls.cleared += 1;
  };
  function Harness() {
    latest = useChannelAgentSessions({
      activeChannel: null,
      activeChannelId: "chan-1",
      agentsLoaded: false,
      clearAgentSessionReturnThread,
      handleOpenThread: () => {},
      managedAgents: [],
      openThreadHeadId: null,
      requireThreadEditResolution: () => true,
      setChannelManagementOpen: () => {},
      setExpandedThreadReplyIds: () => {},
      setOpenAgentSessionChannelId: () => {},
      setOpenAgentSessionPubkey: (value) => calls.session.push(value),
      setOpenThreadHeadId: (value) => calls.thread.push(value),
      setProfilePanelPubkey: () => {},
      setThreadReplyTargetId: () => {},
      setThreadScrollTargetId: () => {},
      ...props,
    });
    return null;
  }
  const container = document.createElement("div");
  const root = createRoot(container);
  await act(async () => root.render(React.createElement(Harness)));
  return {
    calls,
    latest: () => latest,
    unmount: async () => act(async () => root.unmount()),
  };
}

test("a return thread from the URL becomes the pane's back target", async () => {
  const { calls, latest, unmount } = await mount({
    agentSessionReturnThreadId: ROOT,
    openAgentSessionPubkey: AGENT,
  });

  assert.equal(calls.cleared, 1);
  assert.equal(latest().hasAgentSessionReturnTarget, true);
  await act(async () => latest().backFromAgentSession());
  assert.deepEqual(calls.session, [null]);
  assert.deepEqual(calls.thread, [ROOT]);
  await unmount();
});

test("a return thread without an open pane is dropped, not recorded", async () => {
  const { calls, latest, unmount } = await mount({
    agentSessionReturnThreadId: ROOT,
    openAgentSessionPubkey: null,
  });

  assert.equal(calls.cleared, 1);
  assert.equal(latest().hasAgentSessionReturnTarget, false);
  await unmount();
});
