import assert from "node:assert/strict";
import { test } from "node:test";

import {
  relayMediaCandidates,
  relayMediaVariantForSize,
} from "./relayMediaVariants.ts";

const HASH = "a".repeat(64);
const BASE = "https://kuzz.example/media";

test("small avatars try small, then thumb, then the original", () => {
  for (const ext of ["jpg", "png", "webp"]) {
    const original = `${BASE}/${HASH}.${ext}`;
    assert.deepEqual(relayMediaCandidates(original, "small"), [
      `${BASE}/${HASH}.small.jpg`,
      `${BASE}/${HASH}.thumb.jpg`,
      original,
    ]);
  }
});

test("larger avatars try thumb, then the original", () => {
  const original = `${BASE}/${HASH}.png`;
  assert.deepEqual(relayMediaCandidates(original, "thumb"), [
    `${BASE}/${HASH}.thumb.jpg`,
    original,
  ]);
});

test("variants drop a query string from the original", () => {
  assert.equal(
    relayMediaCandidates(`${BASE}/${HASH}.png?v=2`, "thumb")[0],
    `${BASE}/${HASH}.thumb.jpg`,
  );
});

test("GIFs, variants, and non-media URLs only load the original", () => {
  for (const url of [
    `${BASE}/${HASH}.gif`,
    `${BASE}/${HASH}.thumb.jpg`,
    `${BASE}/${HASH}.small.jpg`,
    `${BASE}/${HASH}`,
    `${BASE}/${HASH}.mp4`,
    `https://kuzz.example/files/${HASH}.png`,
    `${BASE}/${"a".repeat(63)}.png`,
    "data:image/svg+xml,%3Csvg%3E",
  ]) {
    assert.deepEqual(relayMediaCandidates(url, "small"), [url], url);
  }
});

test("animated avatar URLs only load the original", () => {
  const animated = `${BASE}/${HASH}.png#kuzz-anim=${encodeURIComponent(
    `${BASE}/${"b".repeat(64)}.png`,
  )}`;
  assert.deepEqual(relayMediaCandidates(animated, "small"), [animated]);
});

test("empty input has nothing to load", () => {
  for (const url of ["", "  ", null, undefined]) {
    assert.deepEqual(relayMediaCandidates(url, "small"), []);
  }
});

test("avatars up to 56 CSS px use the small variant", () => {
  assert.equal(relayMediaVariantForSize(16), "small");
  assert.equal(relayMediaVariantForSize(56), "small");
  assert.equal(relayMediaVariantForSize(57), "thumb");
  assert.equal(relayMediaVariantForSize(80), "thumb");
});
