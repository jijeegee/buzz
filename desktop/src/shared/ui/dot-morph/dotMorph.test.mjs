import assert from "node:assert/strict";
import test from "node:test";

import { DOT_LOADERS } from "./DotMorphLoader.tsx";
import { DotMorph, DotTimeline, DUST, loadDotScene } from "./dotMorph.ts";

test("every loader step names a shape its scene has", () => {
  for (const { scene: name, steps } of Object.values(DOT_LOADERS)) {
    const scene = loadDotScene(name);
    for (const [key] of steps) {
      assert.ok(scene.shapes.has(key), `${name} is missing ${key}`);
    }
    for (const points of scene.shapes.values()) {
      assert.equal(points.length, scene.count * 2);
    }
  }
});

test("shape points stay inside the scene", () => {
  for (const { scene: name } of Object.values(DOT_LOADERS)) {
    const scene = loadDotScene(name);
    for (const [key, points] of scene.shapes) {
      if (key === DUST) continue;
      for (let i = 0; i < points.length; i += 2) {
        assert.ok(points[i] >= 0 && points[i] <= scene.width, `${name}/${key}`);
        assert.ok(
          points[i + 1] >= 0 && points[i + 1] <= scene.height,
          `${name}/${key}`,
        );
      }
    }
  }
});

test("a morph lands exactly on the target shape", () => {
  const scene = loadDotScene("inviteWait");
  const morph = new DotMorph(scene, "k");
  morph.go("p", 1000);
  morph.tick(500);
  assert.equal(morph.settled, false);
  assert.notDeepEqual(morph.positions, scene.shapes.get("p"));
  morph.tick(500);
  assert.equal(morph.settled, true);
  assert.deepEqual(morph.positions, scene.shapes.get("p"));
});

test("a timeline starts on its first shape and then advances in order", () => {
  const scene = loadDotScene("accountSwitch");
  const morph = new DotMorph(scene, "a");
  const timeline = new DotTimeline(morph, DOT_LOADERS.accountSwitch.steps);
  timeline.tick(16);
  assert.deepEqual(morph.positions, scene.shapes.get("a"));
  // a: 700 + 900 ms, then b moves for 700 ms.
  timeline.tick(1600);
  timeline.tick(700);
  assert.deepEqual(morph.positions, scene.shapes.get("b"));
});
