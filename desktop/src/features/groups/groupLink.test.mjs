import assert from "node:assert/strict";
import test from "node:test";

import {
  isValidGroupHandle,
  normalizeGroupHandle,
  parseGroupLink,
  PUBLIC_ADDRESS_INVALID_TEXT,
  PUBLIC_ADDRESS_RESERVED_TEXT,
  PUBLIC_ADDRESS_TAKEN_TEXT,
  publicAddressErrorText,
  publicAddressPrefix,
} from "./groupLink.ts";

const CODE = "AbCdEfGhIjKlMnOpQrStUv"; // 22 URL-safe characters

test("https links with a code or @handle", () => {
  assert.deepEqual(parseGroupLink(`https://kuzz.example/g/${CODE}`), {
    kind: "code",
    code: CODE,
  });
  assert.deepEqual(parseGroupLink("https://kuzz.example/g/@Design_Team"), {
    kind: "handle",
    handle: "design_team",
  });
  assert.deepEqual(parseGroupLink(`http://kuzz.example:8443/g/${CODE}/`), {
    kind: "code",
    code: CODE,
  });
  assert.deepEqual(parseGroupLink("https://kuzz.example/g/%40design"), {
    kind: "handle",
    handle: "design",
  });
});

test("app scheme links: kuzz and a demo build scheme", () => {
  assert.deepEqual(parseGroupLink(`kuzz://join/${CODE}`), {
    kind: "code",
    code: CODE,
  });
  assert.deepEqual(parseGroupLink("kuzz://join/@design"), {
    kind: "handle",
    handle: "design",
  });
  assert.deepEqual(
    parseGroupLink("buzz-demo-preview-11ea9e5994418051://join/@design"),
    { kind: "handle", handle: "design" },
  );
});

test("bare code and bare @handle", () => {
  assert.deepEqual(parseGroupLink(`  ${CODE}  `), { kind: "code", code: CODE });
  assert.deepEqual(parseGroupLink("@design"), {
    kind: "handle",
    handle: "design",
  });
});

test("rejects things that are not group links", () => {
  for (const input of [
    "",
    "   ",
    "design", // too short for a code, no @
    "@ab", // handle too short
    `@${"a".repeat(33)}`,
    "@bad-handle",
    `https://kuzz.example/x/${CODE}`,
    `https://kuzz.example/g/${CODE}/extra`,
    `https://kuzz.example/g/${CODE}?ref=1`,
    `https://kuzz.example/g/${CODE}#frag`,
    `https://user@kuzz.example/g/${CODE}`,
    `ftp://kuzz.example/g/${CODE}`,
    `other://join/${CODE}`,
    `kuzz://open/${CODE}`,
    `kuzz://join/${CODE}/more`,
    `${CODE} ${CODE}`,
    `a/${CODE}`,
    "https://kuzz.example/g/%E0%A4%A",
  ]) {
    assert.equal(parseGroupLink(input), null, input);
  }
});

test("public address prefix and handle rules", () => {
  assert.equal(
    publicAddressPrefix("https://kuzz.example:8443"),
    "kuzz.example:8443/g/@",
  );
  assert.equal(publicAddressPrefix(null), "/g/@");
  assert.equal(normalizeGroupHandle("  @Design_1 "), "design_1");
  assert.equal(isValidGroupHandle("design_1"), true);
  assert.equal(isValidGroupHandle("de"), false);
  assert.equal(isValidGroupHandle("de-sign"), false);
});

test("public address save errors", () => {
  assert.equal(
    publicAddressErrorText(409, "this address is taken"),
    PUBLIC_ADDRESS_TAKEN_TEXT,
  );
  assert.equal(
    publicAddressErrorText(400, "this address is reserved"),
    PUBLIC_ADDRESS_RESERVED_TEXT,
  );
  assert.equal(
    publicAddressErrorText(400, "invalid handle"),
    PUBLIC_ADDRESS_INVALID_TEXT,
  );
  assert.equal(publicAddressErrorText(500, "boom"), null);
  assert.equal(publicAddressErrorText(null, ""), null);
});
