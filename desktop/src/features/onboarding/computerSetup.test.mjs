import assert from "node:assert/strict";
import test from "node:test";

import { accountIsSetUp } from "./computerSetup.ts";

const runtime = (id, status, availability = "available") => ({
  id,
  availability,
  authStatus: { status },
});
const config = (preferred_runtime, provider = null) => ({
  preferred_runtime,
  provider,
});

test("a connected default means the account is set up here", () => {
  assert.equal(
    accountIsSetUp(config("codex"), [runtime("codex", "logged_in")]),
    true,
  );
});

test("a bundled default needs its provider", () => {
  const bundled = [runtime("kuzz-agent", "not_applicable")];
  assert.equal(accountIsSetUp(config("kuzz-agent"), bundled), false);
  assert.equal(
    accountIsSetUp(config("kuzz-agent", "anthropic"), bundled),
    true,
  );
});

test("a connected tool with no default is not set up", () => {
  assert.equal(
    accountIsSetUp(config(null), [runtime("claude", "logged_in")]),
    false,
  );
});

test("a default that lost its sign-in or install is not set up", () => {
  assert.equal(
    accountIsSetUp(config("claude"), [runtime("claude", "logged_out")]),
    false,
  );
  assert.equal(
    accountIsSetUp(config("claude"), [
      runtime("claude", "logged_in", "missing"),
    ]),
    false,
  );
  assert.equal(accountIsSetUp(config("claude"), []), false);
});
