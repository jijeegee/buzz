import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

// Room goal UI follows the per-room goals switch (kind:39000 `goals` tag),
// not a device-local labs flag: the header goal chip renders iff the active
// channel has goals on.

const tauriMock = {
  invoke(command) {
    // The goal chip's live subscription asks for the relay URL; leave it
    // pending so no socket is opened and nothing logs a failure.
    if (command === "get_relay_ws_url") return new Promise(() => {});
    return Promise.reject(new Error(`unmocked Tauri command: ${command}`));
  },
  transformCallback() {
    return Math.random();
  },
};
globalThis.__TAURI_INTERNALS__ = tauriMock;
globalThis.window.__TAURI_INTERNALS__ = tauriMock;
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};

const React = (await import("react")).default;
const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { QueryClient, QueryClientProvider } = await import(
  "@tanstack/react-query"
);
const { CommunitiesProvider } = await import(
  "../../communities/useCommunities.tsx"
);
const { UpdaterProvider } = await import(
  "../../settings/hooks/UpdaterProvider.tsx"
);
const { TooltipProvider } = await import("@/shared/ui/tooltip");
const { ChannelScreenHeader } = await import("./ChannelScreenHeader.tsx");

function channel(overrides = {}) {
  return {
    archivedAt: null,
    channelType: "stream",
    description: "",
    goalsEnabled: true,
    id: "11111111-1111-4111-8111-111111111111",
    // A non-member open channel renders the Join button instead of the
    // members bar, which keeps this mount free of huddle/presence providers.
    isMember: false,
    lastMessageAt: null,
    memberCount: 1,
    memberPubkeys: [],
    name: "general",
    participantPubkeys: [],
    participants: [],
    purpose: null,
    topic: null,
    ttlDeadline: null,
    ttlSeconds: null,
    visibility: "open",
    ...overrides,
  };
}

async function mountHeader(activeChannel) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(
      React.createElement(
        QueryClientProvider,
        { client: queryClient },
        React.createElement(
          CommunitiesProvider,
          null,
          React.createElement(
            UpdaterProvider,
            null,
            React.createElement(
              TooltipProvider,
              null,
              React.createElement(ChannelScreenHeader, {
                activeChannel,
                activeChannelEphemeralDisplay: null,
                activeChannelTitle: activeChannel.name,
                activeDmAvatarUrl: null,
                activeDmHeaderParticipants: [],
                activeDmPresenceStatus: null,
                onManageChannel() {},
                onOpenGoals() {},
                onToggleMembers() {},
                onJoinChannel: async () => {},
              }),
            ),
          ),
        ),
      ),
    );
  });
  return {
    chip: () => container.querySelector("[data-testid='goal-header-chip']"),
    unmount: async () => {
      await act(async () => root.unmount());
      container.remove();
    },
  };
}

test("goal chip renders when the room has goals on", async () => {
  const { chip, unmount } = await mountHeader(channel({ goalsEnabled: true }));
  assert.ok(chip(), "goal chip must render for a goals-on room");
  await unmount();
});

test("goal chip is hidden when the room has goals off", async () => {
  const { chip, unmount } = await mountHeader(channel({ goalsEnabled: false }));
  assert.equal(chip(), null);
  await unmount();
});

test("goal chip is shown in a goals-on DM", async () => {
  const { chip, unmount } = await mountHeader(
    channel({ channelType: "dm", goalsEnabled: true }),
  );
  assert.ok(chip());
  await unmount();
});

// The thread panel and the message "Link to goal" item follow their
// channel's switch the same way; both are too provider-heavy to mount here,
// so pin their gates to the channel field rather than a labs flag.
test("thread panel and link-to-goal item gate on the channel's goalsEnabled", () => {
  const read = (path) =>
    readFileSync(new URL(path, import.meta.url), "utf8").replace(/\s+/g, " ");
  const thread = read("../../messages/ui/MessageThreadPanel.tsx");
  assert.match(
    thread,
    /const goalsEnabled = channel\?\.goalsEnabled === true;/,
  );
  assert.doesNotMatch(thread, /goalTree"\)/);
  const link = read("../../goals/ui/LinkMessageToGoal.tsx");
  assert.match(link, /const channel = useChannelReference\(channelId\);/);
  assert.match(link, /if \(channel\?\.goalsEnabled !== true\) return null;/);
});
