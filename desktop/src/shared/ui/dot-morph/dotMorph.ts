// Dot morph engine shared by the loading screens. A scene is a fixed number
// of square dots that travel between shapes (the K, the wordmark, people and
// robots from kuzz.app). generate-dot-shapes.html samples the shape points
// into dotShapes.json, in tenths of a scene unit.
import shapeData from "./dotShapes.json";

export type DotSceneName = keyof typeof shapeData;

export type DotScene = {
  width: number;
  height: number;
  count: number;
  dotSize: number;
  shapes: Map<string, Float32Array>;
};

/** One timeline step: shape key, transition ms, hold ms. */
export type DotStep = readonly [
  key: string,
  transitionMs: number,
  holdMs: number,
];

export const DUST = "dust";

// Share of dots drawn in the accent color, as on kuzz.app.
const ACCENT_SHARE = 0.05;
// How late (as a share of the transition) the slowest dot starts moving.
const MAX_LAG = 0.45;

export function hash(i: number, k: number): number {
  const v = Math.sin(i * 12.9898 + k * 78.233) * 43758.5453;
  return v - Math.floor(v);
}

const smooth = (t: number) => t * t * (3 - 2 * t);
const clamp = (v: number, lo: number, hi: number) =>
  v < lo ? lo : v > hi ? hi : v;

export function isAccentDot(i: number): boolean {
  return hash(i, 9) <= ACCENT_SHARE;
}

const loadedScenes = new Map<DotSceneName, DotScene>();

/** Returns the scene's shapes, plus scattered "dust"; cached per scene. */
export function loadDotScene(name: DotSceneName): DotScene {
  const cached = loadedScenes.get(name);
  if (cached) return cached;
  const data = shapeData[name];
  const shapes = new Map<string, Float32Array>();
  for (const [key, points] of Object.entries(data.shapes)) {
    shapes.set(
      key,
      Float32Array.from(points as number[], (v) => v / 10),
    );
  }
  const dust = new Float32Array(data.count * 2);
  const cx = data.width / 2;
  const cy = data.height / 2;
  for (let i = 0; i < data.count; i++) {
    dust[2 * i] = cx + (hash(i, 1) - 0.5) * data.width * 1.15 * data.dustSpread;
    dust[2 * i + 1] =
      cy + (hash(i, 2) - 0.5) * data.height * 1.15 * data.dustSpread;
  }
  shapes.set(DUST, dust);
  const scene = {
    width: data.width,
    height: data.height,
    count: data.count,
    dotSize: data.dotSize,
    shapes,
  };
  loadedScenes.set(name, scene);
  return scene;
}

/** Moves a scene's dots between shapes; positions are in scene units. */
export class DotMorph {
  readonly scene: DotScene;
  readonly positions: Float32Array;
  private from: Float32Array;
  private to: Float32Array;
  private progress = 1;
  private duration = 1;

  constructor(scene: DotScene, initial: string) {
    this.scene = scene;
    const start = this.shape(initial);
    this.positions = start.slice();
    this.from = start;
    this.to = start;
  }

  private shape(key: string): Float32Array {
    const shape = this.scene.shapes.get(key);
    if (!shape) throw new Error(`Unknown dot shape: ${key}`);
    return shape;
  }

  /** Starts moving every dot from where it is now to `key`. */
  go(key: string, durationMs: number): void {
    this.from = this.positions.slice();
    this.to = this.shape(key);
    this.progress = durationMs > 0 ? 0 : 1;
    this.duration = Math.max(1, durationMs);
  }

  get settled(): boolean {
    return this.progress >= 1;
  }

  tick(dtMs: number): void {
    this.progress = Math.min(1, this.progress + dtMs / this.duration);
    const { count, height } = this.scene;
    for (let i = 0; i < count; i++) {
      const lag = hash(i, 5) * MAX_LAG;
      const u = smooth(clamp((this.progress - lag) / (1 - lag), 0, 1));
      // Each dot bows sideways on its way, so shapes swirl instead of sliding.
      const bow = Math.sin(u * Math.PI) * height * 0.16 * (hash(i, 6) - 0.5);
      const fx = this.from[2 * i];
      const fy = this.from[2 * i + 1];
      this.positions[2 * i] = fx + (this.to[2 * i] - fx) * u + bow;
      this.positions[2 * i + 1] =
        fy + (this.to[2 * i + 1] - fy) * u - Math.abs(bow) * 0.6;
    }
  }
}

/** Plays steps in order, forever. The first step appears without a transition. */
export class DotTimeline {
  private index = 0;
  private wait = 0;
  readonly morph: DotMorph;
  private readonly steps: readonly DotStep[];

  constructor(morph: DotMorph, steps: readonly DotStep[]) {
    this.morph = morph;
    this.steps = steps;
    this.wait = steps[0][1] + steps[0][2];
    this.index = steps.length > 1 ? 1 : 0;
  }

  tick(dtMs: number): void {
    this.wait -= dtMs;
    if (this.wait <= 0 && this.steps.length > 1) {
      const [key, transition, hold] = this.steps[this.index];
      this.morph.go(key, transition);
      this.wait = transition + hold;
      this.index = (this.index + 1) % this.steps.length;
    }
    this.morph.tick(dtMs);
  }
}
