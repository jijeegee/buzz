/**
 * Behavior tests for Settings → Admin nav visibility.
 *
 * Tests render the real SettingsView and assert on the sidebar DOM.
 *
 * Kuzz contract: the relay-admin ("Admin") nav entry appears only after the
 * server confirms the active identity as an operator or moderator. No
 * origin, a lookup error, a pending lookup or a denied probe all hide it, so
 * ordinary users never see the admin console or its access-denied key dump.
 */

import assert from "node:assert/strict";
import { afterEach, test } from "node:test";

import React from "react";
import { createRoot } from "react-dom/client";
import { act } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

import { SidebarProvider } from "@/shared/ui/sidebar";
import { SettingsView, settingsNavGroups } from "./SettingsView.tsx";

// ── Browser API stubs ─────────────────────────────────────────────────────────
// jsdom does not implement requestAnimationFrame or matchMedia. Stub them so
// SettingsView's `isLoaded` effect and the sidebar's responsive hook don't throw.

if (!globalThis.window.requestAnimationFrame) {
  globalThis.window.requestAnimationFrame = (cb) => {
    setTimeout(cb, 0);
    return 0;
  };
  globalThis.window.cancelAnimationFrame = () => {};
}
if (!globalThis.window.matchMedia) {
  globalThis.window.matchMedia = () => ({
    matches: false,
    addListener: () => {},
    removeListener: () => {},
    addEventListener: () => {},
    removeEventListener: () => {},
    dispatchEvent: () => false,
  });
}

// ── Tauri IPC interceptor ─────────────────────────────────────────────────────

const ipcHandlers = new Map();
function setIpcHandler(cmd, fn) {
  ipcHandlers.set(cmd, fn);
}
function clearIpcHandlers() {
  ipcHandlers.clear();
}

const tauriMock = {
  invoke(cmd, args) {
    const h = ipcHandlers.get(cmd);
    if (h) return h(args);
    return new Promise(() => {}); // pending — prevents unmocked-IPC errors
  },
  transformCallback(_cb) {
    return Math.random();
  },
};
globalThis.__TAURI_INTERNALS__ = tauriMock;
if (globalThis.window && globalThis.window !== globalThis) {
  globalThis.window.__TAURI_INTERNALS__ = tauriMock;
}

// ── Minimal stub props for SettingsView ───────────────────────────────────────

const STUB_PROPS = {
  isUpdatingDesktopNotifications: false,
  notificationErrorMessage: null,
  notificationPermission: "denied",
  notificationSettings: {
    desktopNotificationsEnabled: false,
    homeBadgeEnabled: false,
    notifyWhileViewing: false,
    slotAlerts: {},
  },
  onSetDesktopNotificationsEnabled: async () => false,
  onSetHomeBadgeEnabled: () => {},
  onSetSlotAlertsEnabled: () => {},
  onSetNotifyWhileViewing: () => {},
  onSetAllSlotAlertsEnabled: () => {},
  onSetSoundForSlot: () => {},
};

// ── Harness ───────────────────────────────────────────────────────────────────

function makeQueryClient(pubkeyHex) {
  const qc = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: Infinity } },
  });
  qc.setQueryData(["identity"], { pubkey: pubkeyHex });
  return qc;
}

function mountSettingsView({
  section = "relay-admin",
  onSectionChange = () => {},
} = {}) {
  const qc = makeQueryClient("ab".repeat(32));
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);

  const doRender = async () => {
    await act(async () => {
      root.render(
        React.createElement(
          QueryClientProvider,
          { client: qc },
          React.createElement(
            SidebarProvider,
            {},
            React.createElement(SettingsView, {
              ...STUB_PROPS,
              section,
              onClose: () => {},
              onSectionChange,
            }),
          ),
        ),
      );
    });
  };

  const unmount = async () => {
    await act(async () => {
      root.unmount();
    });
    document.body.removeChild(container);
  };

  return { container, doRender, unmount };
}

async function settle(ms = 50) {
  await act(async () => {
    await new Promise((r) => setTimeout(r, ms));
  });
}

afterEach(() => {
  clearIpcHandlers();
});

// ── Core IPC stubs shared across all nav tests ────────────────────────────────
// These return the "no origin, no discovery" state so the nav resolution hook
// (if it were still active) would resolve to {originSource:"none"}.

function stubNoOrigin() {
  setIpcHandler("get_admin_origin", () => Promise.resolve(null));
  setIpcHandler("admin_discover_origin", () => Promise.resolve(null));
  setIpcHandler("get_relay_origin", () => Promise.resolve(null));
  setIpcHandler("get_relay_members_info", () => Promise.resolve(null));
}

// ── Tests ─────────────────────────────────────────────────────────────────────

function navEntry(container) {
  return container.querySelector("[data-testid='settings-nav-relay-admin']");
}

test("relay-admin-nav-hidden-no-origin: nav is hidden when no origin is saved or discovered", async () => {
  stubNoOrigin();
  const { container, doRender, unmount } = mountSettingsView({
    section: "shortcuts",
  });
  try {
    await doRender();
    await settle();
    assert.equal(navEntry(container), null);
  } finally {
    await unmount();
  }
});

test("relay-admin-nav-hidden-discovery-error: nav is hidden when origin lookups throw", async () => {
  setIpcHandler("get_admin_origin", () =>
    Promise.reject(new Error("storage error")),
  );
  setIpcHandler("admin_discover_origin", () =>
    Promise.reject(new Error("network error")),
  );
  setIpcHandler("get_relay_origin", () => Promise.resolve(null));
  setIpcHandler("get_relay_members_info", () => Promise.resolve(null));
  const { container, doRender, unmount } = mountSettingsView({
    section: "shortcuts",
  });
  try {
    await doRender();
    await settle();
    assert.equal(navEntry(container), null);
  } finally {
    await unmount();
  }
});

test("relay-admin-nav-hidden-pending: nav stays hidden while the access check is pending", async () => {
  setIpcHandler("get_admin_origin", () => new Promise(() => {}));
  setIpcHandler("admin_discover_origin", () => new Promise(() => {}));
  setIpcHandler("get_relay_origin", () => Promise.resolve(null));
  setIpcHandler("get_relay_members_info", () => Promise.resolve(null));
  const { container, doRender, unmount } = mountSettingsView({
    section: "shortcuts",
  });
  try {
    await doRender();
    await settle();
    assert.equal(navEntry(container), null);
  } finally {
    await unmount();
  }
});

test("relay-admin-nav-hidden-denied: nav is hidden when the probe denies this identity", async () => {
  stubNoOrigin();
  setIpcHandler("get_admin_origin", () =>
    Promise.resolve("https://admin.example"),
  );
  setIpcHandler("admin_probe", () => Promise.resolve({ state: "nip98Denied" }));
  const { container, doRender, unmount } = mountSettingsView({
    section: "shortcuts",
  });
  try {
    await doRender();
    await settle(80);
    assert.equal(navEntry(container), null);
  } finally {
    await unmount();
  }
});

test("relay-admin-nav-visible-authorized: nav appears once the probe authorizes an operator", async () => {
  stubNoOrigin();
  setIpcHandler("get_admin_origin", () =>
    Promise.resolve("https://admin.example"),
  );
  setIpcHandler("admin_probe", () =>
    Promise.resolve({ state: "nip98Authorized", role: "operator" }),
  );
  const { container, doRender, unmount } = mountSettingsView({
    section: "shortcuts",
  });
  try {
    await doRender();
    await settle(80);
    assert.ok(navEntry(container), "Admin nav must render for an operator");
  } finally {
    await unmount();
  }
});

test("no-probe-before-save: admin_probe not called while rendering nav with no saved origin", async () => {
  // SettingsView no longer calls the nav resolution hook, so no probe hook
  // runs during nav rendering. Any probe before explicit Save is a trust boundary
  // violation.
  stubNoOrigin();
  let probeCalled = false;
  setIpcHandler("admin_probe", () => {
    probeCalled = true;
    return Promise.resolve({ state: "disabled" });
  });

  const { doRender, unmount } = mountSettingsView();
  try {
    await doRender();
    await settle(80);
    assert.equal(
      probeCalled,
      false,
      "admin_probe must not be called before an explicit Save",
    );
  } finally {
    await unmount();
  }
});

test("settings-nav-groups-contains-relay-admin: relay-admin is wired into Communities nav group", () => {
  const communities = settingsNavGroups.find((g) => g.label === "Communities");
  assert.ok(communities, "Communities group must exist");
  assert.ok(
    communities.sections.includes("relay-admin"),
    `relay-admin must be in Communities; got: ${JSON.stringify(communities.sections)}`,
  );
});
