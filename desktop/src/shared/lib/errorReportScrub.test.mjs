import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  MAX_REPORTS_PER_RUN,
  MAX_TEXT_CHARS,
  createReportGate,
  scrubEvent,
  scrubText,
} from "./errorReportScrub.ts";

describe("scrubText", () => {
  it("removes quoted text, which can be message text", () => {
    const scrubbed = scrubText(
      `Unexpected token 'h', "hello my secret plan" is not valid JSON`,
    );
    assert.ok(!scrubbed.includes("secret plan"), scrubbed);
    assert.ok(scrubbed.startsWith("Unexpected token"));
  });

  it("removes keys, tokens and emails", () => {
    const scrubbed = scrubText(
      [
        "nsec1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq",
        "8cf9a7e7deebdc7d1fe172b9af4c6796b224e43e8f03e80dbd357f6c61fdeff9",
        "someone@example.com",
        "token=abc.def.ghi",
        "eyJhbGciOiJIUzI1NiJ9eyJzdWIiOiIxMjM0NTY3ODkwIn0abcdefgh",
      ].join(" "),
    );
    for (const secret of [
      "nsec1",
      "8cf9a7e7",
      "someone@example.com",
      "abc.def.ghi",
      "eyJhbGci",
    ]) {
      assert.ok(!scrubbed.includes(secret), `${secret} left in ${scrubbed}`);
    }
  });

  it("drops URL queries and keeps hosts", () => {
    assert.equal(
      scrubText("GET https://gateway.kuzz.app/api/x?auth=zzz#frag failed"),
      "GET https://gateway.kuzz.app/api/x failed",
    );
  });

  it("caps length", () => {
    assert.equal(
      Array.from(scrubText("word ".repeat(200))).length,
      MAX_TEXT_CHARS + 1,
    );
  });
});

describe("scrubEvent", () => {
  it("drops identity and context and scrubs exception text", () => {
    const event = scrubEvent({
      user: { email: "someone@example.com" },
      request: { url: "tauri://localhost/?x=1" },
      server_name: "JIHO-PC",
      breadcrumbs: [{ message: "console: hello" }],
      extra: { body: "hello" },
      tags: { pubkey: "abc" },
      exception: {
        values: [
          {
            type: "TypeError",
            value: "cannot read `my message`",
            stacktrace: { frames: [{ function: "f", vars: { text: "hi" } }] },
          },
        ],
      },
    });
    assert.deepEqual(event, {
      exception: {
        values: [
          {
            type: "TypeError",
            value: "cannot read [quoted]",
            stacktrace: { frames: [{ function: "f" }] },
          },
        ],
      },
    });
  });
});

describe("createReportGate", () => {
  const error = (value) => ({
    exception: { values: [{ type: "Error", value }] },
  });

  it("admits the same error once and caps a run", () => {
    const admit = createReportGate();
    assert.equal(admit(error("a")), true);
    assert.equal(admit(error("a")), false);
    for (let n = 1; n < MAX_REPORTS_PER_RUN; n += 1) {
      assert.equal(admit(error(String(n))), true);
    }
    assert.equal(admit(error("one too many")), false);
  });
});
