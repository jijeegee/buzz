/**
 * Per-device robot variants.
 *
 * Every relay device (one app install signed in on one computer or phone)
 * gets a robot: a paint and a silhouette. The owner's apps give each device
 * the next free slot in palette order as soon as they see it (the k-th device
 * gets paint k and shape k, see `nextDeviceRobotAssignments`) and store it as
 * the device's kind:30181 robot, so it never changes afterwards. Until then a
 * device shows a hashed default derived from its id.
 * A desktop that runs agents lists them in the owner's own kind:30180 event
 * for its device (see `shared/api/ownerDevices.ts`), so the
 * agent's owner can tell at a glance which computer the agent runs on, and the
 * device list shows the same robot next to each device.
 *
 * The mapping is shared with the mobile app (`device_robot.dart`) and the
 * Tauri publisher (`device_robot.rs`); `test-fixtures/device-robots.json`
 * pins identical results on every platform. Change all three together.
 */

/** A robot's stroke: one colour, or a diagonal gradient on the 24×24 grid. */
export type DeviceRobotPaint =
  | { name: string; kind: "solid"; color: string }
  | {
      name: string;
      kind: "gradient";
      /** Representative colour where a gradient cannot be drawn. */
      color: string;
      from: readonly [number, number];
      to: readonly [number, number];
      /** `[offset 0..1, colour]`, in order; the ends extend past them. */
      stops: readonly (readonly [number, string])[];
    };

const RAINBOW = [
  "#FF2D2D",
  "#FF8A00",
  "#FFD000",
  "#22C55E",
  "#1E90FF",
  "#4B4BE0",
  "#A040E0",
] as const;

function evenStops(colors: readonly string[], first = 0, span = 1) {
  return colors.map(
    (color, index) =>
      [
        Math.round((first + (index * span) / (colors.length - 1)) * 10_000) /
          10_000,
        color,
      ] as const,
  );
}

/**
 * The robot palette in assignment order: six solid colours far apart in hue,
 * then rainbow, gold and silver. Order matters: a new device takes the first
 * free paint, so the first devices of an account are the easiest to tell
 * apart.
 */
export const DEVICE_ROBOT_PAINTS: readonly DeviceRobotPaint[] = [
  { name: "red", kind: "solid", color: "#F03A3A" },
  { name: "green", kind: "solid", color: "#1FBF4F" },
  { name: "violet", kind: "solid", color: "#8B5CF6" },
  { name: "orange", kind: "solid", color: "#FF7A00" },
  { name: "blue", kind: "solid", color: "#2F7FF2" },
  { name: "yellow", kind: "solid", color: "#F5C400" },
  {
    name: "rainbow",
    kind: "gradient",
    color: "#1E90FF",
    from: [2, 2],
    to: [22, 22],
    // The first and last colours hold the outer 18% so ears and antennae
    // at either end still show them.
    stops: evenStops(RAINBOW, 0.18, 0.64),
  },
  {
    name: "gold",
    kind: "gradient",
    color: "#B8860B",
    from: [0, 0],
    to: [24, 24],
    stops: evenStops(["#8A6200", "#F7D774", "#B8860B", "#FFE9A0", "#9C7A1E"]),
  },
  {
    name: "silver",
    kind: "gradient",
    color: "#9CA3AF",
    from: [0, 0],
    to: [24, 24],
    stops: evenStops(["#6B7280", "#F3F4F6", "#9CA3AF", "#FFFFFF", "#6B7280"]),
  },
];

/** Each paint's representative colour, in palette order. */
export const DEVICE_ROBOT_COLORS = DEVICE_ROBOT_PAINTS.map(
  (paint) => paint.color,
);

/**
 * A paint as a CSS `background` for a square box spanning the 24×24 grid
 * (e.g. a masked chip icon): `135deg` runs corner to corner, so grid offsets
 * map onto that diagonal.
 */
export function deviceRobotPaintCss(paint: DeviceRobotPaint): string {
  if (paint.kind === "solid") return paint.color;
  const start = (paint.from[0] + paint.from[1]) / 48;
  const end = (paint.to[0] + paint.to[1]) / 48;
  const stops = paint.stops.map(
    ([offset, color]) =>
      `${color} ${Math.round((start + offset * (end - start)) * 10_000) / 100}%`,
  );
  return `linear-gradient(135deg, ${stops.join(", ")})`;
}

/** Paints a hashed default may use: the solid colours only. */
const DEFAULT_COLOR_COUNT = 6;

/** Robot silhouettes, rendered by `DeviceRobotIcon`. */
export const DEVICE_ROBOT_SHAPES = [
  "classic",
  "dome",
  "boxy",
  "visor",
  "tall",
  "hex",
] as const;

export type DeviceRobotShape = (typeof DEVICE_ROBOT_SHAPES)[number];

export type DeviceRobotVariant = {
  /** Eight lowercase hex digits (FNV-1a of the device id). */
  tag: string;
  colorIndex: number;
  color: string;
  paint: DeviceRobotPaint;
  shapeIndex: number;
  shape: DeviceRobotShape;
};

const TAG_PATTERN = /^[0-9a-f]{8}$/;

/**
 * FNV-1a (32-bit) of the trimmed, lowercased device id, as 8 hex digits.
 * Hashing publishes no raw device id; `null` for an empty id.
 */
export function deviceRobotTag(
  deviceId: string | null | undefined,
): string | null {
  const normalized = deviceId?.trim().toLowerCase() ?? "";
  if (normalized.length === 0) return null;
  let hash = 0x811c9dc5;
  for (const byte of new TextEncoder().encode(normalized)) {
    hash ^= byte;
    hash = Math.imul(hash, 0x01000193) >>> 0;
  }
  return hash.toString(16).padStart(8, "0");
}

/** The robot for a device tag; `null` when the tag is absent or malformed. */
export function deviceRobotVariantFromTag(
  tag: string | null | undefined,
): DeviceRobotVariant | null {
  if (typeof tag !== "string" || !TAG_PATTERN.test(tag)) return null;
  const value = Number.parseInt(tag, 16);
  const colorIndex = value % DEFAULT_COLOR_COUNT;
  const shapeIndex =
    Math.floor(value / DEFAULT_COLOR_COUNT) % DEVICE_ROBOT_SHAPES.length;
  return {
    tag,
    colorIndex,
    color: DEVICE_ROBOT_PAINTS[colorIndex].color,
    paint: DEVICE_ROBOT_PAINTS[colorIndex],
    shapeIndex,
    shape: DEVICE_ROBOT_SHAPES[shapeIndex],
  };
}

/** The robot for a relay device id (device list rows). */
export function deviceRobotVariantForDevice(
  deviceId: string | null | undefined,
): DeviceRobotVariant | null {
  return deviceRobotVariantFromTag(deviceRobotTag(deviceId));
}

/**
 * The stored robot of one of the owner's devices (kind:30181, content
 * `{"v":2,"shape":"<shape>","color":<DEVICE_ROBOT_PAINTS index>}`): assigned
 * automatically the first time an owner's app sees the device, or picked by
 * the owner. Replaces the hashed default everywhere that device's robot
 * appears. Version 1 indexed an older palette and is ignored, so those devices
 * get a fresh assignment.
 */
export type DeviceRobotOverride = {
  shape: DeviceRobotShape;
  colorIndex: number;
};

/** The override a kind:30181 content carries; `null` when malformed. */
export function parseDeviceRobotContent(
  content: string,
): DeviceRobotOverride | null {
  let parsed: unknown;
  try {
    parsed = JSON.parse(content);
  } catch {
    return null;
  }
  if (typeof parsed !== "object" || parsed === null) return null;
  const { v, shape, color } = parsed as {
    v?: unknown;
    shape?: unknown;
    color?: unknown;
  };
  if (v !== 2) return null;
  if (
    typeof shape !== "string" ||
    !(DEVICE_ROBOT_SHAPES as readonly string[]).includes(shape)
  ) {
    return null;
  }
  if (
    typeof color !== "number" ||
    !Number.isInteger(color) ||
    color < 0 ||
    color >= DEVICE_ROBOT_COLORS.length
  ) {
    return null;
  }
  return { shape: shape as DeviceRobotShape, colorIndex: color };
}

/** The kind:30181 content for an override. */
export function deviceRobotContent(override: DeviceRobotOverride): string {
  return JSON.stringify({
    v: 2,
    shape: override.shape,
    color: override.colorIndex,
  });
}

/**
 * The robot a device shows: the owner's override when there is a valid one,
 * else the hashed default. The tag stays the hashed one, since it identifies
 * the device (e.g. for its name), not its look.
 */
export function resolveDeviceRobotVariant(
  deviceId: string | null | undefined,
  override: DeviceRobotOverride | null | undefined,
): DeviceRobotVariant | null {
  const base = deviceRobotVariantForDevice(deviceId);
  if (!base || !override) return base;
  const shapeIndex = DEVICE_ROBOT_SHAPES.indexOf(override.shape);
  const paint = DEVICE_ROBOT_PAINTS[override.colorIndex];
  if (shapeIndex < 0 || paint === undefined) return base;
  return {
    tag: base.tag,
    colorIndex: override.colorIndex,
    color: paint.color,
    paint,
    shapeIndex,
    shape: override.shape,
  };
}

/** The robot of the k-th device (0-based) of an account. */
export function deviceRobotSlot(slot: number): DeviceRobotOverride {
  return {
    colorIndex: slot % DEVICE_ROBOT_PAINTS.length,
    shape: DEVICE_ROBOT_SHAPES[slot % DEVICE_ROBOT_SHAPES.length],
  };
}

/**
 * Robots to store for the owner's devices that have none yet, in device
 * order (oldest first): each takes the first slot whose paint no stored robot
 * uses, so the first devices of an account get the paints that differ most.
 * Robots of devices no longer listed (logged out) keep their paint reserved.
 * Once every paint is taken, slots continue past the palette. Deterministic,
 * so two of the owner's apps assigning at once write the same robots.
 */
export function nextDeviceRobotAssignments(
  deviceIds: readonly string[],
  stored: ReadonlyMap<string, DeviceRobotOverride>,
): [deviceId: string, override: DeviceRobotOverride][] {
  const used = new Set([...stored.values()].map((robot) => robot.colorIndex));
  let assigned = stored.size;
  const result: [string, DeviceRobotOverride][] = [];
  for (const raw of deviceIds) {
    const deviceId = raw.trim().toLowerCase();
    if (!deviceId || stored.has(deviceId)) continue;
    if (result.some(([id]) => id === deviceId)) continue;
    let slot = -1;
    for (let index = 0; index < DEVICE_ROBOT_PAINTS.length; index += 1) {
      if (!used.has(index)) {
        slot = index;
        break;
      }
    }
    if (slot < 0) slot = assigned;
    used.add(slot % DEVICE_ROBOT_PAINTS.length);
    assigned += 1;
    result.push([deviceId, deviceRobotSlot(slot)]);
  }
  return result;
}

export type AgentBadge =
  | { kind: "device"; deviceId: string; variant: DeviceRobotVariant }
  | { kind: "owner"; ownerPubkey: string }
  | { kind: "default" };

/**
 * What sits in an agent's robot slot for `viewerPubkey`: its owner sees the
 * robot of the device it runs on; everyone else sees an owner mark (the
 * owner's avatar with a robot badge) so they can tell whose agent it is; an
 * agent without a verified owner, or an owner viewing an agent with no known
 * host device, keeps the default robot. `hostDeviceId` is the relay device id
 * from the owner's own host-device map.
 */
export function agentBadge(input: {
  hostDeviceId: string | null | undefined;
  /** The owner's robot choice for that device, if any. */
  robotOverride?: DeviceRobotOverride | null;
  ownerPubkey: string | null | undefined;
  viewerPubkey: string | null | undefined;
}): AgentBadge {
  const owner = input.ownerPubkey?.toLowerCase();
  if (!owner) return { kind: "default" };
  const viewer = input.viewerPubkey?.toLowerCase();
  if (!viewer) return { kind: "default" };
  if (owner !== viewer) return { kind: "owner", ownerPubkey: owner };
  const deviceId = input.hostDeviceId?.trim().toLowerCase() ?? "";
  const variant = resolveDeviceRobotVariant(deviceId, input.robotOverride);
  return variant ? { kind: "device", deviceId, variant } : { kind: "default" };
}
