/**
 * Per-device robot variants.
 *
 * Every relay device (one Google sign-in on one computer or phone) gets a
 * deterministic robot: a colour and a silhouette derived from the device id.
 * A desktop that runs an agent stamps its device tag into the agent's signed
 * kind:0 profile (`buzz_host_device`), so the agent's owner can tell at a
 * glance which computer the agent runs on, and the device list shows the same
 * robot next to each device.
 *
 * The mapping is shared with the mobile app (`device_robot.dart`) and the
 * Tauri publisher (`device_robot.rs`); `test-fixtures/device-robots.json`
 * pins identical results on every platform. Change all three together.
 */

/** kind:0 content field carrying the host device tag of an agent. */
export const HOST_DEVICE_FIELD = "buzz_host_device";

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
  /** Eight lowercase hex digits; also the published `buzz_host_device`. */
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
 * The robot an agent shows to `viewerPubkey`: only the agent's verified
 * NIP-OA owner sees the device robot, so another account's agent cannot
 * pose as one of the viewer's own computers. Everyone else gets `null`
 * (the default robot).
 */
export function agentDeviceRobotVariant(input: {
  hostDevice: string | null | undefined;
  ownerPubkey: string | null | undefined;
  viewerPubkey: string | null | undefined;
}): DeviceRobotVariant | null {
  const owner = input.ownerPubkey?.toLowerCase();
  const viewer = input.viewerPubkey?.toLowerCase();
  if (!owner || !viewer || owner !== viewer) return null;
  return deviceRobotVariantFromTag(input.hostDevice);
}
