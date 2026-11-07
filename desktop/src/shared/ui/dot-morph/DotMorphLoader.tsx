import { useLayoutEffect, useRef } from "react";

import { cn } from "@/shared/lib/cn";

import {
  DotMorph,
  type DotSceneName,
  type DotStep,
  DotTimeline,
  isAccentDot,
  loadDotScene,
} from "./dotMorph";

// Accent dots use the kuzz.app yellows, brighter on dark backgrounds.
const ACCENT_LIGHT = "#F0A500";
const ACCENT_DARK = "#FFD84D";
// Longest frame step, so a stalled tab does not jump a whole transition.
const MAX_FRAME_MS = 50;

/** Ready-made loading scenes, timed as in the approved mockup. */
export const DOT_LOADERS = {
  // Scattered dots gather into the Kuzz wordmark.
  appStart: {
    scene: "appStart",
    steps: [
      ["dust", 500, 500],
      ["w", 1300, 2000],
    ],
  },
  // One face scatters and gathers into the next.
  accountSwitch: {
    scene: "accountSwitch",
    steps: [
      ["a", 700, 900],
      ["b", 700, 900],
      ["c", 700, 900],
    ],
  },
  // My side crosses to the other side and back, then both stay.
  huddle: {
    scene: "huddle",
    steps: [
      ["l", 500, 300],
      ["r", 800, 300],
      ["l", 800, 300],
      ["r", 800, 300],
      ["b", 500, 1600],
    ],
  },
  // A slow K → person → robot cycle for screens that can wait a while.
  inviteWait: {
    scene: "inviteWait",
    steps: [
      ["k", 1000, 1600],
      ["p", 1000, 1600],
      ["r", 1000, 1600],
    ],
  },
} as const satisfies Record<
  string,
  { scene: DotSceneName; steps: readonly DotStep[] }
>;

export type DotLoaderName = keyof typeof DOT_LOADERS;

function prefersReducedMotion(): boolean {
  return (
    typeof window.matchMedia === "function" &&
    window.matchMedia("(prefers-reduced-motion: reduce)").matches
  );
}

/**
 * A loading animation drawn in dots, in `currentColor` with a few accent dots.
 * With reduced motion it shows the scene's resting shape without moving.
 */
export function DotMorphLoader({
  loader,
  className,
}: {
  loader: DotLoaderName;
  className?: string;
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const { scene: sceneName, steps } = DOT_LOADERS[loader];
  const scene = loadDotScene(sceneName);

  // A layout effect draws the first frame before the browser paints, so the
  // loader never flashes an empty box.
  useLayoutEffect(() => {
    const canvas = canvasRef.current;
    const context = canvas?.getContext("2d");
    if (!canvas || !context) return;

    const still = prefersReducedMotion();
    const restingKey = steps[steps.length - 1][0];
    const morph = new DotMorph(scene, still ? restingKey : steps[0][0]);
    const timeline = new DotTimeline(morph, steps);
    const scale = window.devicePixelRatio || 1;
    canvas.width = Math.round(scene.width * scale);
    canvas.height = Math.round(scene.height * scale);

    const draw = () => {
      const foreground = getComputedStyle(canvas).color;
      const accent = canvas.closest(".dark") ? ACCENT_DARK : ACCENT_LIGHT;
      const size = scene.dotSize * scale;
      const half = size / 2;
      const points = morph.positions;
      context.setTransform(1, 0, 0, 1, 0, 0);
      context.clearRect(0, 0, canvas.width, canvas.height);
      for (const [color, accentPass] of [
        [foreground, false],
        [accent, true],
      ] as const) {
        context.fillStyle = color;
        context.beginPath();
        for (let i = 0; i < scene.count; i++) {
          if (isAccentDot(i) !== accentPass) continue;
          context.rect(
            points[2 * i] * scale - half,
            points[2 * i + 1] * scale - half,
            size,
            size,
          );
        }
        context.fill();
      }
    };

    if (still) {
      draw();
      return;
    }

    let frame = 0;
    let last = performance.now();
    const step = (now: number) => {
      timeline.tick(Math.min(MAX_FRAME_MS, now - last));
      last = now;
      draw();
      frame = window.requestAnimationFrame(step);
    };
    draw();
    frame = window.requestAnimationFrame(step);
    return () => window.cancelAnimationFrame(frame);
  }, [scene, steps]);

  return (
    <span
      aria-hidden="true"
      className={cn("block", className)}
      data-dot-loader={loader}
    >
      <canvas
        className="block"
        ref={canvasRef}
        style={{ width: scene.width, height: scene.height }}
      />
    </span>
  );
}
