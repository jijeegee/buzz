import assert from "node:assert/strict";
import test from "node:test";

// The browser tab can't be watched, so coming back to Kuzz frees the
// "Continue with ChatGPT" button; the earlier sign-in settles silently.

const signIns = [];
const tauriMock = {
  invoke(command) {
    switch (command) {
      case "chatgpt_plan_status":
        return Promise.resolve({
          signedIn: false,
          planEnabled: false,
          email: null,
        });
      case "chatgpt_plan_sign_in":
        return new Promise((resolve, reject) => {
          signIns.push({ resolve, reject });
        });
      default:
        return Promise.reject(new Error(`unmocked Tauri command: ${command}`));
    }
  },
  transformCallback() {
    return Math.random();
  },
  unregisterCallback() {},
};
globalThis.__TAURI_INTERNALS__ = tauriMock;
globalThis.window.__TAURI_INTERNALS__ = tauriMock;
// The user clicks from a focused Kuzz window; jsdom reports no focus.
document.hasFocus = () => true;

const { fireEvent } = await import("@testing-library/react");
const React = (await import("react")).default;
const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { ContinueWithChatGptButton } = await import("./ChatGptPlanControls.tsx");
const { CHATGPT_RETURN_GRACE_MS } = await import("../useChatGptPlan.ts");

const wait = (ms) =>
  act(() => new Promise((resolve) => setTimeout(resolve, ms)));

async function mount() {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(React.createElement(ContinueWithChatGptButton));
  });
  const button = () =>
    container.querySelector("[data-testid='chatgpt-plan-sign-in']");
  return {
    container,
    button,
    unmount: async () => {
      await act(async () => root.unmount());
      container.remove();
    },
  };
}

test("returning to Kuzz frees the button and a stale attempt stays silent", async () => {
  signIns.length = 0;
  const { container, button, unmount } = await mount();
  await act(async () => fireEvent.click(button()));
  assert.equal(signIns.length, 1);
  assert.equal(button().disabled, true);

  // Focus alone (the browser never took it) keeps waiting.
  await act(async () => window.dispatchEvent(new Event("focus")));
  await wait(CHATGPT_RETURN_GRACE_MS + 100);
  assert.equal(button().disabled, true);

  await act(async () => {
    window.dispatchEvent(new Event("blur"));
    window.dispatchEvent(new Event("focus"));
  });
  await wait(CHATGPT_RETURN_GRACE_MS + 100);
  assert.equal(button().disabled, false);
  assert.equal(button().textContent, "Continue with ChatGPT");

  await act(async () => fireEvent.click(button()));
  assert.equal(signIns.length, 2);
  await act(async () => signIns[0].reject("a newer sign-in replaced this one"));
  await wait(10);
  assert.equal(button().disabled, true);
  assert.equal(container.querySelector("[role='alert']"), null);
  await unmount();
});

test("leaving Kuzz again within the grace keeps waiting", async () => {
  signIns.length = 0;
  const { button, unmount } = await mount();
  await act(async () => fireEvent.click(button()));
  await act(async () => {
    window.dispatchEvent(new Event("blur"));
    window.dispatchEvent(new Event("focus"));
  });
  await wait(CHATGPT_RETURN_GRACE_MS / 2);
  await act(async () => window.dispatchEvent(new Event("blur")));
  await wait(CHATGPT_RETURN_GRACE_MS);
  assert.equal(button().disabled, true);
  await unmount();
});
