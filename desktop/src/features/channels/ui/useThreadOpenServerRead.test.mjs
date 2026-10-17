import assert from "node:assert/strict";
import test from "node:test";

import { newestShownThreadAt } from "./useThreadOpenServerRead.ts";

const reply = (createdAt) => ({ message: { createdAt } });

test("the thread read goes up to the newest message shown", () => {
  assert.equal(newestShownThreadAt(null, [reply(9)]), null);
  assert.equal(newestShownThreadAt({ createdAt: 5 }, []), 5);
  assert.equal(
    newestShownThreadAt({ createdAt: 5 }, [reply(7), reply(12), reply(8)]),
    12,
  );
});
