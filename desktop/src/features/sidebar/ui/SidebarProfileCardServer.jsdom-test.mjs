import assert from "node:assert/strict";
import { afterEach, test } from "node:test";

// The sidebar profile card and its menu with a built-in server: no server
// name or community actions, and "Invite people" only for members who can
// invite. Without a built-in server (E2E/upstream) the switcher stays.

globalThis.requestAnimationFrame ??= (callback) => setTimeout(callback, 0);
globalThis.window.requestAnimationFrame ??= globalThis.requestAnimationFrame;
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};
globalThis.window.ResizeObserver ??= globalThis.ResizeObserver;

const tauriMock = {
  invoke(command) {
    switch (command) {
      case "list_accounts":
        return Promise.resolve([]);
      case "plugin:event|listen":
        return Promise.resolve(1);
      case "plugin:event|unlisten":
        return Promise.resolve(null);
      default:
        return Promise.reject(new Error(`unmocked Tauri command: ${command}`));
    }
  },
  transformCallback: () => Math.random(),
  unregisterCallback() {},
};
globalThis.__TAURI_INTERNALS__ = tauriMock;
globalThis.window.__TAURI_INTERNALS__ = tauriMock;
globalThis.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
globalThis.window.__TAURI_EVENT_PLUGIN_INTERNALS__ =
  globalThis.__TAURI_EVENT_PLUGIN_INTERNALS__;

const React = (await import("react")).default;
const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { fireEvent } = await import("@testing-library/react");
const { QueryClient, QueryClientProvider } = await import(
  "@tanstack/react-query"
);
const { CommunitiesProvider } = await import(
  "../../communities/useCommunities.tsx"
);
const { setBuiltInServerForTests } = await import(
  "../../communities/builtInServer.ts"
);
const { myRelayMembershipLookupQueryKey } = await import(
  "../../community-members/hooks.ts"
);
const { TooltipProvider } = await import("../../../shared/ui/tooltip.tsx");
const { SidebarProfileCard } = await import("./SidebarProfileCard.tsx");

const BUILT_IN = "wss://jihobuzz.duckdns.org:8443";
const community = {
  id: "c1",
  name: "jihobuzz",
  relayUrl: BUILT_IN,
  addedAt: "2026-01-01T00:00:00.000Z",
};

let root = null;
let container = null;

afterEach(async () => {
  if (root) {
    await act(async () => root.unmount());
  }
  container?.remove();
  root = null;
  container = null;
  setBuiltInServerForTests(undefined);
  localStorage.clear();
});

async function renderCard({ builtIn, role, userStatus }) {
  localStorage.setItem("buzz-communities", JSON.stringify([community]));
  localStorage.setItem("buzz-active-community-id", community.id);
  setBuiltInServerForTests(builtIn);
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  queryClient.setQueryData(myRelayMembershipLookupQueryKey, {
    membershipRequired: true,
    membership: role ? { pubkey: "a".repeat(64), role } : null,
  });
  const settingsOpened = [];
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  await act(async () => {
    root.render(
      React.createElement(
        QueryClientProvider,
        { client: queryClient },
        React.createElement(
          CommunitiesProvider,
          null,
          React.createElement(
            TooltipProvider,
            null,
            React.createElement(SidebarProfileCard, {
              activeCommunity: community,
              communities: [community],
              onClearUserStatus: () => {},
              onLeaveCommunity: async () => undefined,
              onOpenAddCommunity: () => {},
              onOpenSettings: (section) => settingsOpened.push(section),
              onRemoveCommunityFromDevice: async () => {},
              onSetUserStatus: () => {},
              onSwitchCommunity: () => {},
              onUpdateCommunity: () => {},
              resolvedDisplayName: "Jiho",
              selfPresenceStatus: "online",
              selfUserStatus: userStatus,
            }),
          ),
        ),
      ),
    );
  });
  return { settingsOpened };
}

async function openMenu() {
  const button = document.querySelector(
    '[data-testid="sidebar-profile-avatar-button"]',
  );
  assert.ok(button, "avatar button rendered");
  await act(async () => {
    fireEvent.click(button);
  });
  const menu = document.querySelector('[data-testid="profile-popover"]');
  assert.ok(menu, "profile menu opened");
  return menu;
}

test("built-in server: no server name and no community actions", async () => {
  await renderCard({ builtIn: BUILT_IN, role: "member" });
  assert.equal(container.textContent.includes("jihobuzz"), false);
  assert.equal(container.textContent.includes("🐝"), false);

  const menu = await openMenu();
  assert.equal(
    document.querySelector('[data-testid="community-switcher"]'),
    null,
  );
  assert.equal(
    document.querySelector('[data-testid="profile-popover-community"]'),
    null,
  );
  for (const text of [
    "jihobuzz",
    "Copy community URL",
    "Community settings",
    "Add a community",
    "Remove from this device",
    "Invite people",
  ]) {
    assert.equal(menu.textContent.includes(text), false, text);
  }
});

test("built-in server: owners and admins get Invite people", async () => {
  for (const role of ["owner", "admin"]) {
    const { settingsOpened } = await renderCard({ builtIn: BUILT_IN, role });
    await openMenu();
    const invite = document.querySelector(
      '[data-testid="profile-popover-invite-people"]',
    );
    assert.ok(invite, `${role} sees Invite people`);
    await act(async () => {
      fireEvent.click(invite);
      await new Promise((resolve) => setTimeout(resolve, 5));
    });
    assert.deepEqual(settingsOpened, ["community-members"]);
    await act(async () => root.unmount());
    root = null;
    container.remove();
    container = null;
  }
});

test("built-in server: a status shows without the server name on hover", async () => {
  await renderCard({
    builtIn: BUILT_IN,
    role: "member",
    userStatus: { text: "Focusing", emoji: "" },
  });
  assert.ok(
    document.querySelector('[data-testid="sidebar-profile-user-status"]'),
  );
  assert.equal(container.textContent.includes("jihobuzz"), false);
});

test("without a built-in server the upstream switcher stays", async () => {
  await renderCard({ builtIn: null, role: "admin" });
  assert.ok(container.textContent.includes("jihobuzz"));
  await openMenu();
  assert.ok(document.querySelector('[data-testid="community-switcher"]'));
  assert.equal(
    document.querySelector('[data-testid="profile-popover-invite-people"]'),
    null,
  );
});
