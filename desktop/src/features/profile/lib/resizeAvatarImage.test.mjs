import assert from "node:assert/strict";
import { test } from "node:test";

import {
  AVATAR_JPEG_QUALITY,
  AVATAR_MAX_DIMENSION,
  avatarTargetSize,
  resizeAvatarImage,
} from "./resizeAvatarImage.ts";

function fakeDeps({ width, height, alpha = 255, encodedSize = 10 }) {
  const calls = { decode: 0, toBlob: [], canvas: null, closed: false };
  return {
    calls,
    deps: {
      decode: async () => {
        calls.decode += 1;
        return {
          width,
          height,
          source: {},
          close: () => {
            calls.closed = true;
          },
        };
      },
      createCanvas: () => {
        const canvas = {
          width: 0,
          height: 0,
          getContext: () => ({
            drawImage: () => {},
            getImageData: (_x, _y, w, h) => {
              const data = new Uint8ClampedArray(w * h * 4).fill(255);
              data[3] = alpha;
              return { data };
            },
          }),
          toBlob: (callback, type, quality) => {
            calls.toBlob.push({ type, quality });
            callback(new Blob([new Uint8Array(encodedSize)], { type }));
          },
        };
        calls.canvas = canvas;
        return canvas;
      },
    },
  };
}

function fileOf(type, size = 1000, name = "photo.bin") {
  return new File([new Uint8Array(size)], name, { type });
}

test("caps the longest side at 640 and keeps the aspect ratio", () => {
  assert.equal(AVATAR_MAX_DIMENSION, 640);
  assert.deepEqual(avatarTargetSize(2048, 2048), { width: 640, height: 640 });
  assert.deepEqual(avatarTargetSize(4000, 3000), { width: 640, height: 480 });
  assert.deepEqual(avatarTargetSize(1000, 3000), { width: 213, height: 640 });
});

test("never upscales small images", () => {
  assert.deepEqual(avatarTargetSize(300, 200), { width: 300, height: 200 });
});

test("downsizes a large opaque PNG to a 640px JPEG", async () => {
  const { calls, deps } = fakeDeps({ width: 2048, height: 2048 });
  const out = await resizeAvatarImage(
    fileOf("image/png", 5_000_000, "group.png"),
    deps,
  );
  assert.equal(out.type, "image/jpeg");
  assert.equal(out.name, "group.jpg");
  assert.equal(calls.canvas.width, 640);
  assert.equal(calls.canvas.height, 640);
  assert.deepEqual(calls.toBlob, [
    { type: "image/jpeg", quality: AVATAR_JPEG_QUALITY },
  ]);
  assert.equal(calls.closed, true);
});

test("keeps a transparent PNG as PNG at the reduced size", async () => {
  const { calls, deps } = fakeDeps({ width: 3000, height: 1500, alpha: 0 });
  const out = await resizeAvatarImage(fileOf("image/png", 5_000_000), deps);
  assert.equal(out.type, "image/png");
  assert.equal(calls.canvas.width, 640);
  assert.equal(calls.canvas.height, 320);
  assert.deepEqual(calls.toBlob, [{ type: "image/png", quality: undefined }]);
});

test("uploads a small JPEG unchanged", async () => {
  const { calls, deps } = fakeDeps({ width: 600, height: 400 });
  const file = fileOf("image/jpeg");
  assert.equal(await resizeAvatarImage(file, deps), file);
  assert.equal(calls.toBlob.length, 0);
});

test("uploads a small transparent PNG unchanged", async () => {
  const { calls, deps } = fakeDeps({ width: 512, height: 512, alpha: 10 });
  const file = fileOf("image/png");
  assert.equal(await resizeAvatarImage(file, deps), file);
  assert.equal(calls.toBlob.length, 0);
});

test("keeps a small opaque PNG unless JPEG is smaller", async () => {
  const bigger = fakeDeps({ width: 512, height: 512, encodedSize: 5000 });
  const file = fileOf("image/png", 1000);
  assert.equal(await resizeAvatarImage(file, bigger.deps), file);

  const smaller = fakeDeps({ width: 512, height: 512, encodedSize: 100 });
  const out = await resizeAvatarImage(file, smaller.deps);
  assert.equal(out.type, "image/jpeg");
  assert.equal(smaller.calls.canvas.width, 512);
});

test("passes GIFs through without decoding", async () => {
  const { calls, deps } = fakeDeps({ width: 4000, height: 4000 });
  const file = fileOf("image/gif");
  assert.equal(await resizeAvatarImage(file, deps), file);
  assert.equal(calls.decode, 0);
});

test("falls back to the original when decoding fails", async () => {
  const file = fileOf("image/webp");
  const out = await resizeAvatarImage(file, {
    decode: async () => {
      throw new Error("undecodable");
    },
  });
  assert.equal(out, file);
});
