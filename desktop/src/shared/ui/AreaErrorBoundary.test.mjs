import assert from "node:assert/strict";
import { after, afterEach, before, test } from "node:test";

import { JSDOM } from "jsdom";

const dom = new JSDOM("<!doctype html><html><body></body></html>", {
  url: "http://localhost",
});
const originalConsoleError = console.error;

before(() => {
  Object.assign(globalThis, {
    document: dom.window.document,
    HTMLElement: dom.window.HTMLElement,
    IS_REACT_ACT_ENVIRONMENT: true,
    window: dom.window,
  });
  dom.window.matchMedia = () => ({
    matches: false,
    addEventListener() {},
    removeEventListener() {},
  });
});

afterEach(async () => {
  const { cleanup } = await import("@testing-library/react");
  cleanup();
  console.error = originalConsoleError;
});

after(() => dom.window.close());

test("a throwing area shows a retry notice and leaves siblings rendered", async () => {
  console.error = () => {};
  const { createElement, Fragment } = await import("react");
  const { render, screen } = await import("@testing-library/react");
  const { AreaErrorBoundary } = await import("./AreaErrorBoundary.tsx");
  function BrokenSidebar() {
    throw new TypeError("undefined is not an object");
  }

  render(
    createElement(
      Fragment,
      null,
      createElement(
        AreaErrorBoundary,
        { area: "sidebar" },
        createElement(BrokenSidebar),
      ),
      createElement(
        AreaErrorBoundary,
        { area: "main" },
        createElement("p", null, "Conversation is visible"),
      ),
    ),
  );

  assert.ok(screen.getByTestId("area-error-sidebar"));
  assert.ok(screen.getByRole("button", { name: "Try again" }));
  assert.ok(screen.getByText("Conversation is visible"));
  assert.equal(document.body.textContent.includes("undefined"), false);
});

test("an area recovers on retry and when its reset key changes", async () => {
  console.error = () => {};
  const { createElement } = await import("react");
  const { fireEvent, render, screen } = await import("@testing-library/react");
  const { AreaErrorBoundary } = await import("./AreaErrorBoundary.tsx");
  let shouldThrow = true;
  function Flaky() {
    if (shouldThrow) throw new Error("boom");
    return createElement("p", null, "Area is back");
  }
  const tree = (resetKey) =>
    createElement(
      AreaErrorBoundary,
      { area: "main", resetKey },
      createElement(Flaky),
    );

  const { rerender } = render(tree("/a"));
  assert.ok(screen.getByTestId("area-error-main"));

  rerender(tree("/b"));
  assert.ok(screen.getByTestId("area-error-main"));
  shouldThrow = false;
  rerender(tree("/c"));
  assert.ok(screen.getByText("Area is back"));

  shouldThrow = true;
  rerender(tree("/d"));
  assert.ok(screen.getByTestId("area-error-main"));
  shouldThrow = false;
  fireEvent.click(screen.getByRole("button", { name: "Try again" }));
  assert.ok(screen.getByText("Area is back"));
});
