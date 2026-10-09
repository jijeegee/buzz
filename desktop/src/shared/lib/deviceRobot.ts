/**
 * Per-device robot variants.
 *
 * Every relay device (one Google sign-in on one computer or phone) gets a
 * deterministic robot: a colour and a silhouette derived from the device id.
 * A desktop that runs agents lists them in the owner's own kind:30180 event
 * for its device (see `shared/api/ownerDevices.ts`), so the
 * agent's owner can tell at a glance which computer the agent runs on, and the
 * device list shows the same robot next to each device.
 *
 * The mapping is shared with the mobile app (`device_robot.dart`) and the
 * Tauri publisher (`device_robot.rs`); `test-fixtures/device-robots.json`
 * pins identical results on every platform. Change all three together.
 */

/** Robot stroke colours, each ≥3:1 against both light and dark surfaces. */
export const DEVICE_ROBOT_COLORS = [
  "#D93D42", // red
  "#C2620A", // orange
  "#2E8540", // green
  "#0F8478", // teal
  "#1F78C8", // blue
  "#5A5FD8", // indigo
  "#9150C8", // violet
  "#C93A8A", // pink
] as const;

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
  const colorIndex = value % DEVICE_ROBOT_COLORS.length;
  const shapeIndex =
    Math.floor(value / DEVICE_ROBOT_COLORS.length) % DEVICE_ROBOT_SHAPES.length;
  return {
    tag,
    colorIndex,
    color: DEVICE_ROBOT_COLORS[colorIndex],
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
 * The owner's own choice of robot for one of their devices (kind:30181,
 * content `{"v":1,"shape":"<shape>","color":<palette index>}`), replacing the
 * hashed default everywhere that device's robot appears.
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
  if (v !== 1) return null;
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
    v: 1,
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
  const color = DEVICE_ROBOT_COLORS[override.colorIndex];
  if (shapeIndex < 0 || color === undefined) return base;
  return {
    tag: base.tag,
    colorIndex: override.colorIndex,
    color,
    shapeIndex,
    shape: override.shape,
  };
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
