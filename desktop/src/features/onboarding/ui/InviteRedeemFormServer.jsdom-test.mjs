import assert from "node:assert/strict";
import { afterEach, test } from "node:test";

// "Have an invite?" with a built-in server: only invite links for that server
// are redeemed; any other host is refused before anything contacts it.

globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};
globalThis.window.ResizeObserver ??= globalThis.ResizeObserver;
globalThis.window.matchMedia ??= () => ({
  matches: false,
  addEventListener() {},
  removeEventListener() {},
  addListener() {},
  removeListener() {},
});

const fetched = [];
globalThis.fetch = async (url) => {
  fetched.push(String(url));
  return new Response(null, { status: 404 });
};
globalThis.window.fetch = globalThis.fetch;

const React = (await import("react")).default;
const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { fireEvent } = await import("@testing-library/react");
const { setBuiltInServerForTests } = await import(
  "../../communities/builtInServer.ts"
);
const { InviteRedeemForm } = await import("./InviteRedeemForm.tsx");

const BUILT_IN = "wss://jihobuzz.duckdns.org:8443";

let root = null;
let container = null;

afterEach(async () => {
  if (root) await act(async () => root.unmount());
  container?.remove();
  root = null;
  container = null;
  fetched.length = 0;
  setBuiltInServerForTests(undefined);
});

async function submitInvite(builtIn, link) {
  setBuiltInServerForTests(builtIn);
  const redeemed = [];
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  await act(async () => {
    root.render(
      React.createElement(InviteRedeemForm, {
        defaultRelayUrl: BUILT_IN,
        error: null,
        isRedeeming: false,
        onCancel: () => {},
        onRedeem: (relayWsUrl, code) => redeemed.push({ relayWsUrl, code }),
      }),
    );
  });
  const input = container.querySelector('[data-testid="invite-redeem-input"]');
  assert.ok(input);
  await act(async () => {
    fireEvent.change(input, { target: { value: link } });
  });
  await act(async () => {
    fireEvent.submit(input.closest("form"));
    await new Promise((resolve) => setTimeout(resolve, 20));
  });
  return redeemed;
}

test("an invite for another service is refused without contacting it", async () => {
  const redeemed = await submitInvite(
    BUILT_IN,
    "https://other.example/invite/abc123",
  );
  assert.deepEqual(redeemed, []);
  assert.ok(
    container.textContent.includes("This invite is for a different service."),
  );
  assert.equal(
    fetched.some((url) => url.includes("other.example")),
    false,
  );
});

test("an invite for the built-in server is redeemed", async () => {
  const redeemed = await submitInvite(
    BUILT_IN,
    "https://jihobuzz.duckdns.org:8443/invite/abc123",
  );
  assert.equal(redeemed.length, 1);
  assert.equal(redeemed[0].code, "abc123");
  assert.equal(
    container.textContent.includes("This invite is for a different service."),
    false,
  );
});

test("without a built-in server any host is redeemed (upstream)", async () => {
  const redeemed = await submitInvite(
    null,
    "https://other.example/invite/abc123",
  );
  assert.equal(redeemed.length, 1);
});
