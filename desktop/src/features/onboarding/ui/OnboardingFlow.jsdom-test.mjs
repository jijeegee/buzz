import assert from "node:assert/strict";
import test from "node:test";

const calls = [];
const mock = {
  invoke(command) {
    calls.push(command);
    if (command === "plugin:event|listen") return Promise.resolve(1);
    if (command === "plugin:event|unlisten") return Promise.resolve(null);
    return new Promise(() => {});
  },
  transformCallback: () => 1,
  unregisterCallback() {},
};
globalThis.__TAURI_INTERNALS__ = window.__TAURI_INTERNALS__ = mock;
globalThis.__TAURI_EVENT_PLUGIN_INTERNALS__ =
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
globalThis.requestAnimationFrame ??= () => 0;
globalThis.cancelAnimationFrame ??= () => {};
window.matchMedia ??= () => ({
  matches: false,
  addEventListener() {},
  removeEventListener() {},
});

const React = (await import("react")).default;
const { act } = React;
const { render, fireEvent, cleanup } = await import("@testing-library/react");
const { QueryClient, QueryClientProvider } = await import(
  "@tanstack/react-query"
);
const { CommunitiesProvider } = await import(
  "@/features/communities/useCommunities.tsx"
);
const { OnboardingFlow } = await import("./OnboardingFlow.tsx");
const { useFirstRunOnboardingGate } = await import("../hooks.ts");

function FlowWithRealGate({ settled = true, profile }) {
  const gate = useFirstRunOnboardingGate({
    currentPubkey: "a".repeat(64),
    identityIsFetching: false,
    identityLost: false,
    identityStatus: "success",
    isSharedIdentity: false,
    profileHasEvent: false,
    profileIsFetching: false,
    profileStatus: "success",
  });
  if (gate.stage === "ready")
    return React.createElement("main", { "data-testid": "app-ready" }, "App");
  return React.createElement(OnboardingFlow, {
    actions: { complete: gate.complete, skipForNow: gate.skipForNow },
    initialProfile: { profile },
    initialProfileDecisionSettled: settled,
  });
}

async function mount(settled = true, profile) {
  calls.length = 0;
  window.localStorage.clear();
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  if (profile) client.setQueryData(["profile"], profile);
  await act(async () =>
    render(
      React.createElement(
        QueryClientProvider,
        { client },
        React.createElement(
          CommunitiesProvider,
          null,
          React.createElement(FlowWithRealGate, { settled, profile }),
        ),
      ),
    ),
  );
  return client;
}

for (const control of ["onboarding-back", "onboarding-skip"]) {
  test(`${control} skips optional profile setup into the app without signing out`, async () => {
    const client = await mount();
    const button = document.querySelector(`[data-testid='${control}']`);
    assert.ok(button);
    if (control === "onboarding-skip")
      assert.equal(button.textContent.trim(), "Set up later and start");
    await act(async () => fireEvent.click(button));
    assert.ok(
      document.querySelector("[data-testid='app-ready']"),
      "the real onboarding gate becomes ready",
    );
    assert.equal(
      document.querySelector("[data-testid='community-change-overlay']"),
      null,
    );
    assert.equal(calls.includes("logout"), false);
    assert.equal(calls.includes("sign_out"), false);
    assert.equal(
      calls.includes("update_profile"),
      false,
      "blank optional profile isn't published",
    );
    cleanup();
    client.clear();
  });

  test(`${control} keeps the saved profile when an edited name is not submitted`, async () => {
    const profile = {
      pubkey: "a".repeat(64),
      displayName: "Google Name",
      avatarUrl: "https://example.test/avatar.png",
      about: null,
      nip05Handle: null,
      ownerPubkey: null,
      hasProfileEvent: false,
    };
    const client = await mount(true, profile);
    const input = document.querySelector(
      "[data-testid='onboarding-display-name']",
    );
    assert.equal(input.value, profile.displayName);
    await act(async () =>
      fireEvent.change(input, { target: { value: "Unsaved name" } }),
    );
    assert.equal(input.value, "Unsaved name");
    await act(async () =>
      fireEvent.click(document.querySelector(`[data-testid='${control}']`)),
    );
    assert.ok(document.querySelector("[data-testid='app-ready']"));
    assert.deepEqual(client.getQueryData(["profile"]), profile);
    assert.equal(calls.includes("update_profile"), false);
    assert.equal(calls.includes("logout"), false);
    assert.equal(calls.includes("sign_out"), false);
    cleanup();
    client.clear();
  });
}

test("unsettled profile disables both skip controls", async () => {
  const client = await mount(false);
  for (const control of ["onboarding-back", "onboarding-skip"]) {
    const button = document.querySelector(`[data-testid='${control}']`);
    assert.equal(button.disabled, true);
    await act(async () => fireEvent.click(button));
  }
  assert.equal(document.querySelector("[data-testid='app-ready']"), null);
  cleanup();
  client.clear();
});
