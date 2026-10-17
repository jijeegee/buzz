import assert from "node:assert/strict";
import test from "node:test";

import { ThreadUnreadGuard } from "./threadUnreadGuard.ts";

test("reads up to the mark are held back so the open panel cannot clear it", () => {
  const guard = new ThreadUnreadGuard();
  guard.mark("root", 1_000);
  assert.equal(guard.allowRead("thread:root", 990), false);
  assert.equal(guard.allowRead("thread:root", 1_000), false);
  // Other contexts are never held.
  assert.equal(guard.allowRead("thread:other", 990), true);
  assert.equal(guard.allowRead("channel-id", 990), true);
});

test("reading a message newer than the mark lifts the guard", () => {
  const guard = new ThreadUnreadGuard();
  guard.mark("root", 1_000);
  assert.equal(guard.allowRead("thread:root", 1_001), true);
  assert.equal(guard.allowRead("thread:root", 900), true);
});

test("an explicit Mark as read or an account reset lifts it", () => {
  const guard = new ThreadUnreadGuard();
  guard.mark("root", 1_000);
  guard.clear("root");
  assert.equal(guard.allowRead("thread:root", 990), true);
  guard.mark("root", 1_000);
  guard.reset();
  assert.equal(guard.allowRead("thread:root", 990), true);
});
