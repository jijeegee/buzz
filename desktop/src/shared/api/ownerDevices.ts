import type { RelayEvent } from "@/shared/api/types";
import {
  KIND_AGENT_HOST_DEVICES,
  KIND_DEVICE_ROBOT,
} from "@/shared/constants/kinds";
import {
  type DeviceRobotOverride,
  parseDeviceRobotContent,
} from "@/shared/lib/deviceRobot";

/**
 * The signed-in user's own per-device records, both author-only on the relay
 * and keyed by `d` = relay device id:
 *
 * - kind:30180 — which agents the device runs,
 *   content `{"v":1,"agents":["<agent pubkey>", ...]}`;
 * - kind:30181 — the robot the owner picked for the device,
 *   content `{"v":1,"shape":"<shape>","color":<palette index>}`.
 */

const AGENT_PUBKEY_PATTERN = /^[0-9a-f]{64}$/;

export type StoredDeviceRobotOverride = DeviceRobotOverride & {
  /** `created_at` of the winning event, so a re-pick can supersede it. */
  createdAt: number;
};

export type OwnerDevices = {
  /** Agent pubkey (lowercase hex) → relay device id (lowercase). */
  hostDevices: ReadonlyMap<string, string>;
  /** Relay device id (lowercase) → the owner's robot choice. */
  robotOverrides: ReadonlyMap<string, StoredDeviceRobotOverride>;
};

export const EMPTY_OWNER_DEVICES: OwnerDevices = {
  hostDevices: new Map(),
  robotOverrides: new Map(),
};

export function normalizeDeviceId(value: string | null | undefined) {
  return typeof value === "string" ? value.trim().toLowerCase() : "";
}

function deviceIdOf(event: RelayEvent): string | null {
  const deviceId = normalizeDeviceId(
    event.tags.find((tag) => tag[0] === "d")?.[1],
  );
  return deviceId.length > 0 ? deviceId : null;
}

/** The agent pubkeys an event lists, or `null` when its content is malformed. */
export function parseAgentHostDevicesContent(content: string): string[] | null {
  let parsed: unknown;
  try {
    parsed = JSON.parse(content);
  } catch {
    return null;
  }
  if (typeof parsed !== "object" || parsed === null) return null;
  const { v, agents } = parsed as { v?: unknown; agents?: unknown };
  if (v !== 1 || !Array.isArray(agents)) return null;
  const pubkeys: string[] = [];
  for (const agent of agents) {
    if (typeof agent !== "string") continue;
    const pubkey = agent.trim().toLowerCase();
    if (AGENT_PUBKEY_PATTERN.test(pubkey)) pubkeys.push(pubkey);
  }
  return pubkeys;
}

/** NIP-01 replaceable order: later `created_at` wins, then the lower id. */
function isNewer(candidate: RelayEvent, current: RelayEvent): boolean {
  return (
    candidate.created_at > current.created_at ||
    (candidate.created_at === current.created_at && candidate.id < current.id)
  );
}

/**
 * Newest well-formed event of `kind` per device, parsed. Events by anyone but
 * `owner`, without a `d` tag, or whose content `parse` rejects are ignored
 * outright, so a malformed head never hides the previous valid one.
 */
function newestPerDevice<T>(
  events: readonly RelayEvent[],
  owner: string,
  kind: number,
  parse: (content: string) => T | null,
): Map<string, { event: RelayEvent; value: T }> {
  const heads = new Map<string, { event: RelayEvent; value: T }>();
  for (const event of events) {
    if (event.kind !== kind) continue;
    if (event.pubkey.toLowerCase() !== owner) continue;
    const deviceId = deviceIdOf(event);
    if (!deviceId) continue;
    const current = heads.get(deviceId);
    if (current && !isNewer(event, current.event)) continue;
    const value = parse(event.content);
    if (value === null) continue;
    heads.set(deviceId, { event, value });
  }
  return heads;
}

/**
 * Agent → device map from `ownerPubkey`'s kind:30180 events. Only each
 * device's newest well-formed event counts (an older revision that still
 * lists an agent the device has since dropped must not resurrect it), and an
 * agent listed by several devices goes to the newest of them.
 */
export function buildAgentHostDeviceMap(
  events: readonly RelayEvent[],
  ownerPubkey: string,
): Map<string, string> {
  const heads = newestPerDevice(
    events,
    ownerPubkey.trim().toLowerCase(),
    KIND_AGENT_HOST_DEVICES,
    parseAgentHostDevicesContent,
  );
  const newestFirst = [...heads.entries()].sort(([, a], [, b]) =>
    isNewer(a.event, b.event) ? -1 : isNewer(b.event, a.event) ? 1 : 0,
  );
  const map = new Map<string, string>();
  for (const [deviceId, { value: agents }] of newestFirst) {
    for (const agent of agents) {
      if (!map.has(agent)) map.set(agent, deviceId);
    }
  }
  return map;
}

/**
 * Device → robot override from `ownerPubkey`'s kind:30181 events; newest
 * well-formed event per device wins, unknown shapes and out-of-range colours
 * are ignored (the device keeps its hashed default).
 */
export function buildDeviceRobotOverrides(
  events: readonly RelayEvent[],
  ownerPubkey: string,
): Map<string, StoredDeviceRobotOverride> {
  const heads = newestPerDevice(
    events,
    ownerPubkey.trim().toLowerCase(),
    KIND_DEVICE_ROBOT,
    parseDeviceRobotContent,
  );
  const map = new Map<string, StoredDeviceRobotOverride>();
  for (const [deviceId, { event, value }] of heads) {
    map.set(deviceId, { ...value, createdAt: event.created_at });
  }
  return map;
}

export function buildOwnerDevices(
  events: readonly RelayEvent[],
  ownerPubkey: string,
): OwnerDevices {
  return {
    hostDevices: buildAgentHostDeviceMap(events, ownerPubkey),
    robotOverrides: buildDeviceRobotOverrides(events, ownerPubkey),
  };
}
