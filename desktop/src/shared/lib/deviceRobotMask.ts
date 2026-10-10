import type { OwnerDevices } from "@/shared/api/ownerDevices";
import {
  type DeviceRobotShape,
  type DeviceRobotVariant,
  resolveDeviceRobotVariant,
} from "@/shared/lib/deviceRobot";
import {
  DEVICE_ROBOT_GEOMETRY,
  type DeviceRobotPrimitive,
} from "@/shared/lib/deviceRobotGeometry";
import { normalizePubkey } from "@/shared/lib/pubkey";

/** Class that swaps an agent chip's CSS bot glyph for its device robot. */
export const AGENT_DEVICE_ROBOT_CHIP_CLASS = "agent-device-robot";

function primitiveMarkup(shape: DeviceRobotPrimitive): string {
  switch (shape.k) {
    case "rect":
      return `<rect x='${shape.x}' y='${shape.y}' width='${shape.w}' height='${shape.h}' rx='${shape.r}'/>`;
    case "line":
      return `<line x1='${shape.x1}' y1='${shape.y1}' x2='${shape.x2}' y2='${shape.y2}'/>`;
    case "circle":
      return `<circle cx='${shape.cx}' cy='${shape.cy}' r='${shape.r}'/>`;
    case "dot":
      return `<circle cx='${shape.cx}' cy='${shape.cy}' r='${shape.r}' fill='%23000' stroke='none'/>`;
    case "poly":
      return `<polygon points='${shape.points.join(" ")}'/>`;
  }
}

const maskUrlCache = new Map<DeviceRobotShape, string>();

/**
 * The robot silhouette as a CSS `mask` image, matching `DeviceRobotIcon`'s
 * stroke. Chips paint their icon as a masked `::before`, so a robot rides in
 * that slot without adding text the chip copy logic would have to skip.
 */
export function deviceRobotMaskUrl(shape: DeviceRobotShape): string {
  const cached = maskUrlCache.get(shape);
  if (cached) return cached;
  const body = DEVICE_ROBOT_GEOMETRY[shape].map(primitiveMarkup).join("");
  const svg = `<svg xmlns='http://www.w3.org/2000/svg' width='24' height='24' viewBox='0 0 24 24' fill='none' stroke='%23000' stroke-width='2' stroke-linecap='round' stroke-linejoin='round'>${body}</svg>`;
  const url = `url("data:image/svg+xml,${svg.replace(/</g, "%3C").replace(/>/g, "%3E")}")`;
  maskUrlCache.set(shape, url);
  return url;
}

/** Custom properties the `agent-device-robot` chip CSS reads. */
export function deviceRobotChipStyle(
  variant: DeviceRobotVariant,
): Record<string, string> {
  return {
    "--agent-robot-mask": deviceRobotMaskUrl(variant.shape),
    "--agent-robot-color": variant.color,
  };
}

/** `deviceRobotChipStyle` as an inline `style` attribute string. */
export function deviceRobotChipStyleText(variant: DeviceRobotVariant): string {
  return Object.entries(deviceRobotChipStyle(variant))
    .map(([name, value]) => `${name}: ${value}`)
    .join("; ");
}

/**
 * The robot of the device running `agentPubkey`, from the viewer's own device
 * records. Those records only cover the viewer's agents, so anyone else's
 * agent resolves to `null` and keeps the default bot, as on mobile.
 */
export function ownAgentDeviceRobot(
  agentPubkey: string | null | undefined,
  ownerDevices: OwnerDevices,
): DeviceRobotVariant | null {
  const pubkey = normalizePubkey(agentPubkey ?? "");
  if (!pubkey) return null;
  const deviceId = ownerDevices.hostDevices.get(pubkey);
  if (!deviceId) return null;
  return resolveDeviceRobotVariant(
    deviceId,
    ownerDevices.robotOverrides.get(deviceId),
  );
}
