import type { DeviceRobotShape } from "./deviceRobot";

/**
 * Robot silhouettes on a 24×24 grid, stroked 2 units wide with round caps
 * and joins (the lucide icon grid, so robots sit where the old `Bot` icon
 * did). `dot` is filled; everything else is stroked. Mirrored by mobile's
 * `device_robot.dart` and pinned in `test-fixtures/device-robots.json`.
 */
export type DeviceRobotPrimitive =
  | { k: "rect"; x: number; y: number; w: number; h: number; r: number }
  | { k: "line"; x1: number; y1: number; x2: number; y2: number }
  | { k: "circle"; cx: number; cy: number; r: number }
  | { k: "dot"; cx: number; cy: number; r: number }
  | { k: "poly"; points: number[] };

export const DEVICE_ROBOT_GEOMETRY: Record<
  DeviceRobotShape,
  readonly DeviceRobotPrimitive[]
> = {
  // Square head, single antenna, side ears, slit eyes.
  classic: [
    { k: "rect", x: 4, y: 8, w: 16, h: 12, r: 2 },
    { k: "line", x1: 12, y1: 8, x2: 12, y2: 4 },
    { k: "dot", cx: 12, cy: 3.5, r: 1.5 },
    { k: "line", x1: 2, y1: 14, x2: 4, y2: 14 },
    { k: "line", x1: 20, y1: 14, x2: 22, y2: 14 },
    { k: "line", x1: 9, y1: 13, x2: 9, y2: 15 },
    { k: "line", x1: 15, y1: 13, x2: 15, y2: 15 },
  ],
  // Round head, two antennae, round eyes.
  dome: [
    { k: "rect", x: 5, y: 8, w: 14, h: 13, r: 6.5 },
    { k: "line", x1: 9, y1: 8.5, x2: 7, y2: 4 },
    { k: "line", x1: 15, y1: 8.5, x2: 17, y2: 4 },
    { k: "dot", cx: 9.5, cy: 14.5, r: 1.5 },
    { k: "dot", cx: 14.5, cy: 14.5, r: 1.5 },
  ],
  // Sharp box head, side bolts, flat eyes and mouth.
  boxy: [
    { k: "rect", x: 5, y: 6, w: 14, h: 14, r: 0.5 },
    { k: "line", x1: 2, y1: 10, x2: 2, y2: 16 },
    { k: "line", x1: 22, y1: 10, x2: 22, y2: 16 },
    { k: "line", x1: 8.5, y1: 11, x2: 10.5, y2: 11 },
    { k: "line", x1: 13.5, y1: 11, x2: 15.5, y2: 11 },
    { k: "line", x1: 9, y1: 16, x2: 15, y2: 16 },
  ],
  // Wide pill head, visor band, T antenna.
  visor: [
    { k: "rect", x: 3, y: 9, w: 18, h: 11, r: 5.5 },
    { k: "line", x1: 7.5, y1: 14.5, x2: 16.5, y2: 14.5 },
    { k: "line", x1: 12, y1: 9, x2: 12, y2: 5 },
    { k: "line", x1: 9.5, y1: 5, x2: 14.5, y2: 5 },
  ],
  // Tall head, tall eyes, ears, mouth.
  tall: [
    { k: "rect", x: 6, y: 3, w: 12, h: 18, r: 3 },
    { k: "line", x1: 10, y1: 8.5, x2: 10, y2: 11.5 },
    { k: "line", x1: 14, y1: 8.5, x2: 14, y2: 11.5 },
    { k: "line", x1: 10, y1: 16, x2: 14, y2: 16 },
    { k: "line", x1: 3, y1: 12, x2: 6, y2: 12 },
    { k: "line", x1: 18, y1: 12, x2: 21, y2: 12 },
  ],
  // Hexagon head, ring eyes.
  hex: [
    { k: "poly", points: [12, 3, 20, 7.5, 20, 16.5, 12, 21, 4, 16.5, 4, 7.5] },
    { k: "circle", cx: 9, cy: 12, r: 1.5 },
    { k: "circle", cx: 15, cy: 12, r: 1.5 },
    { k: "line", x1: 10.5, y1: 16.5, x2: 13.5, y2: 16.5 },
  ],
};
