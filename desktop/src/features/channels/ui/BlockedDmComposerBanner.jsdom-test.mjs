import assert from "node:assert/strict";
import test from "node:test";

// A 1:1 DM with someone I blocked shows "You blocked <name>." with Unblock.
// Unblock uses the same Tauri command as Settings › Blocked users, and the
// banner leaves as soon as the block list updates.

const ME = "a".repeat(64);
const BOB = "b".repeat(64);
const CAROL = "c".repeat(64);

const calls = [];
let serverBlocked = [];
let unblockBehavior;
const tauriMock = {
  invoke(command, args) {
    calls.push({ command, args });
    switch (command) {
      case "list_blocked_people":
        return Promise.resolve(serverBlocked);
      case "set_person_blocked":
        if (unblockBehavior) return unblockBehavior(args);
        serverBlocked = serverBlocked.filter((p) => p.pubkey !== args.pubkey);
        return Promise.resolve(null);
      default:
        return Promise.resolve(null);
    }
  },
  transformCallback() {
    return Math.random();
  },
  unregisterCallback() {},
};
globalThis.__TAURI_INTERNALS__ = tauriMock;
globalThis.window.__TAURI_INTERNALS__ = tauriMock;
window.matchMedia ??= () => ({
  matches: false,
  addEventListener() {},
  removeEventListener() {},
  addListener() {},
  removeListener() {},
});

const { fireEvent } = await import("@testing-library/react");
const React = (await import("react")).default;
const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { QueryClient, QueryClientProvider } = await import(
  "@tanstack/react-query"
);
const { BlockedDmComposerBanner, blockedDmBannerText } = await import(
  "./BlockedDmComposerBanner.tsx"
);
const { BlockedPeopleProvider, blockedPeopleQueryKey, useBlockedPeople } =
  await import("../../people/BlockedPeopleContext.tsx");
const { blockedDmPeer } = await import("../../people/lib/blockedDm.ts");

function dm(participantPubkeys, participants = []) {
  return { channelType: "dm", participantPubkeys, participants };
}

// The same decision ChannelPane makes: banner only in a 1:1 DM with someone
// I blocked; the composer is disabled exactly while it shows.
function Harness({ channel, profiles }) {
  const { isBlocked } = useBlockedPeople();
  const peer = blockedDmPeer(channel, ME, isBlocked);
  return React.createElement(
    React.Fragment,
    null,
    React.createElement("textarea", {
      "data-testid": "composer",
      disabled: peer !== null,
    }),
    peer
      ? React.createElement(BlockedDmComposerBanner, {
          channel,
          currentPubkey: ME,
          peer,
          profiles,
        })
      : null,
  );
}

async function mount({ channel, blocked = [BOB], profiles } = {}) {
  serverBlocked = blocked.map((pubkey) => ({ pubkey, createdAt: null }));
  calls.length = 0;
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  queryClient.setQueryData(["identity"], { pubkey: ME });
  queryClient.setQueryData(blockedPeopleQueryKey(ME), [...serverBlocked]);
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(
      React.createElement(
        QueryClientProvider,
        { client: queryClient },
        React.createElement(
          BlockedPeopleProvider,
          null,
          React.createElement(Harness, {
            channel: channel ?? dm([ME, BOB], ["", "Bob"]),
            profiles,
          }),
        ),
      ),
    );
  });
  const find = (testId) => container.querySelector(`[data-testid='${testId}']`);
  return {
    find,
    unmount: async () => {
      await act(async () => root.unmount());
      container.remove();
    },
  };
}

test("banner text uses the name, or a plain line without one", () => {
  assert.equal(blockedDmBannerText("Bob"), "You blocked Bob.");
  assert.equal(blockedDmBannerText(null), "You blocked this user.");
});

test("1:1 DM with a blocked person: banner with name, composer disabled", async () => {
  const view = await mount({
    profiles: { [BOB]: { displayName: "Bobby", avatarUrl: null } },
  });
  const banner = view.find("composer-blocked-banner");
  assert.ok(banner, "banner missing");
  assert.match(banner.textContent, /You blocked Bobby\./);
  assert.doesNotMatch(banner.textContent, /b{8}/, "no key in the banner");
  assert.equal(view.find("composer").disabled, true);
  await view.unmount();
});

test("no known name falls back to 'You blocked this user.'", async () => {
  const view = await mount({ channel: dm([ME, BOB]) });
  assert.match(
    view.find("composer-blocked-banner").textContent,
    /You blocked this user\./,
  );
  await view.unmount();
});

test("Unblock calls the unblock command; banner leaves, composer opens", async () => {
  const view = await mount();
  await act(async () => {
    fireEvent.click(view.find("composer-blocked-unblock"));
  });
  assert.deepEqual(
    calls.find((call) => call.command === "set_person_blocked")?.args,
    { pubkey: BOB, blocked: false },
  );
  assert.equal(view.find("composer-blocked-banner"), null);
  assert.equal(view.find("composer").disabled, false);
  await view.unmount();
});

test("a failed unblock keeps the banner and the composer locked", async () => {
  unblockBehavior = () => Promise.reject(new Error("offline"));
  try {
    const view = await mount();
    await act(async () => {
      fireEvent.click(view.find("composer-blocked-unblock"));
    });
    assert.ok(view.find("composer-blocked-banner"));
    assert.equal(view.find("composer").disabled, true);
    await view.unmount();
  } finally {
    unblockBehavior = undefined;
  }
});

test("no banner in a group DM or a 1:1 DM with someone not blocked", async () => {
  const group = await mount({ channel: dm([ME, BOB, CAROL]) });
  assert.equal(group.find("composer-blocked-banner"), null);
  assert.equal(group.find("composer").disabled, false);
  await group.unmount();

  const other = await mount({ channel: dm([ME, CAROL]) });
  assert.equal(other.find("composer-blocked-banner"), null);
  await other.unmount();
});
