import type { AgentPairing, HermesProfile } from "@/shared/api/agentPairing";
import type { AuthDevice, AuthDeviceAgent } from "@/shared/api/tokenAuth";

/** How often the connect screen asks whether the code was used. */
export const PAIRING_POLL_MS = 2_000;

/** The relay's platform of a computer paired with `kuzz pair`. */
export const EXTERNAL_PLATFORM = "external";

/**
 * When a code received at `receivedAtMs` expires on this computer's clock:
 * the relay's `expiresIn` seconds from then, so a wrong local clock does not
 * change the countdown.
 */
export function pairingDeadline(
  receivedAtMs: number,
  expiresIn: number,
): number {
  return Number.isFinite(expiresIn) && expiresIn > 0
    ? receivedAtMs + expiresIn * 1000
    : receivedAtMs;
}

/** Whole seconds until `deadlineMs`, never below zero. */
export function secondsLeft(deadlineMs: number, nowMs: number): number {
  if (!Number.isFinite(deadlineMs)) return 0;
  return Math.max(0, Math.ceil((deadlineMs - nowMs) / 1000));
}

/** `9:42`, `0:05`. */
export function formatCountdown(seconds: number): string {
  const whole = Math.max(0, Math.floor(seconds));
  const minutes = Math.floor(whole / 60);
  const rest = whole % 60;
  return `${minutes}:${rest.toString().padStart(2, "0")}`;
}

/** The claim that connected `pairingId`, if it did. */
export function connectedClaim(
  pairings: readonly AgentPairing[],
  pairingId: string,
): AgentPairing | null {
  return (
    pairings.find(
      (pairing) =>
        pairing.id === pairingId &&
        pairing.state === "connected" &&
        pairing.agentPubkey !== null &&
        pairing.deviceId !== null,
    ) ?? null
  );
}

export type PairingPollOptions = {
  pairingId: string;
  expiresAtMs: number;
  intervalMs?: number;
  list: () => Promise<AgentPairing[]>;
  now: () => number;
  schedule: (run: () => void, ms: number) => unknown;
  cancel: (handle: unknown) => void;
  onConnected: (claim: AgentPairing) => void;
  onExpired: () => void;
};

/**
 * Ask every `intervalMs` whether `pairingId` was claimed, until it connects
 * (`onConnected`), expires (`onExpired`), or the returned stop is called
 * (the screen closed or a new code replaced it). One request at a time; a
 * failed request is retried at the next tick, and expiry ends the loop.
 * Nothing is reported after stop.
 */
export function startPairingPoll(options: PairingPollOptions): () => void {
  const interval = options.intervalMs ?? PAIRING_POLL_MS;
  let stopped = false;
  let handle: unknown = null;

  const expired = () => options.now() >= options.expiresAtMs;
  const finish = (report: () => void) => {
    stopped = true;
    report();
  };

  const tick = async () => {
    handle = null;
    if (stopped) return;
    if (expired()) {
      finish(options.onExpired);
      return;
    }
    try {
      const pairings = await options.list();
      if (stopped) return;
      const claim = connectedClaim(pairings, options.pairingId);
      if (claim) {
        finish(() => options.onConnected(claim));
        return;
      }
      const mine = pairings.find((pairing) => pairing.id === options.pairingId);
      if (mine?.state === "expired") {
        finish(options.onExpired);
        return;
      }
    } catch {
      // A failed check is asked again next time; expiry still ends the loop.
    }
    if (stopped) return;
    if (expired()) {
      finish(options.onExpired);
      return;
    }
    handle = options.schedule(() => void tick(), interval);
  };

  handle = options.schedule(() => void tick(), interval);
  return () => {
    stopped = true;
    if (handle !== null) options.cancel(handle);
    handle = null;
  };
}

/** An external agent the app saw connect, kept past the relay's one-day list. */
export type RememberedPairing = {
  agentPubkey: string;
  deviceId: string;
  connectedAt: string | null;
  /** Connected by this app's Hermes card. */
  hermes?: true;
};

export type ExternalAgent = {
  pubkey: string;
  connectedAt: string | null;
};

export type ExternalAgentGroup = {
  /** The external device. */
  deviceId: string;
  /** The computer's name: its signed-in device's, when there is one. */
  deviceName: string;
  /** The signed-in device on this computer, else `deviceId`. */
  hostDeviceId: string;
  agents: ExternalAgent[];
  /** The relay lists `agents`, so each can be disconnected alone. */
  listed: boolean;
};

/** The connected claims in `pairings`, as remembered entries. */
export function connectedPairings(
  pairings: readonly AgentPairing[],
): RememberedPairing[] {
  return pairings.flatMap((pairing) =>
    pairing.state === "connected" && pairing.agentPubkey && pairing.deviceId
      ? [
          {
            agentPubkey: pairing.agentPubkey.toLowerCase(),
            deviceId: pairing.deviceId,
            connectedAt: pairing.claimedAt,
          },
        ]
      : [],
  );
}

/**
 * `remembered` plus the new connected claims of `pairings`, one entry per
 * agent and computer. When `liveDeviceIds` is given (the device list loaded),
 * entries of disconnected computers are dropped.
 */
export function mergeRemembered(
  remembered: readonly RememberedPairing[],
  pairings: readonly AgentPairing[],
  liveDeviceIds?: ReadonlySet<string>,
): RememberedPairing[] {
  const byKey = new Map<string, RememberedPairing>();
  for (const entry of [...remembered, ...connectedPairings(pairings)]) {
    if (liveDeviceIds && !liveDeviceIds.has(entry.deviceId)) continue;
    const key = `${entry.deviceId}|${entry.agentPubkey}`;
    const known = byKey.get(key);
    const hermes = known?.hermes === true || entry.hermes === true;
    byKey.set(key, {
      agentPubkey: entry.agentPubkey,
      deviceId: entry.deviceId,
      connectedAt: known?.connectedAt ?? entry.connectedAt,
      ...(hermes ? { hermes: true as const } : {}),
    });
  }
  return [...byKey.values()];
}

/**
 * The account's paired computers (devices with the `external` platform, in
 * the relay's order), each with the agents known to be connected on it.
 */
export function groupExternalAgents(
  devices: readonly AuthDevice[],
  pairings: readonly AgentPairing[],
  remembered: readonly RememberedPairing[],
): ExternalAgentGroup[] {
  const entries = mergeRemembered(remembered, pairings);
  return devices
    .filter((device) => device.platform === EXTERNAL_PLATFORM)
    .map((device) => {
      const host = loginDeviceOf(device, devices);
      // The relay's list decides who is connected; remembered entries add
      // the time, and stand in only when the relay lists none.
      const listed = (device.agents ?? []).map(({ pubkey }) =>
        pubkey.toLowerCase(),
      );
      const agents = new Map<string, ExternalAgent>(
        listed.map((pubkey) => [pubkey, { pubkey, connectedAt: null }]),
      );
      for (const entry of entries) {
        if (entry.deviceId !== device.id) continue;
        const agent = agents.get(entry.agentPubkey);
        if (agent) {
          agent.connectedAt ??= entry.connectedAt;
        } else if (listed.length === 0) {
          agents.set(entry.agentPubkey, {
            pubkey: entry.agentPubkey,
            connectedAt: entry.connectedAt,
          });
        }
      }
      return {
        deviceId: device.id,
        deviceName: host?.name ?? device.name,
        hostDeviceId: host?.id ?? device.id,
        agents: [...agents.values()],
        listed: listed.length > 0,
      };
    });
}

/** The listed signed-in device on the same computer as `device`, if any. */
function loginDeviceOf(
  device: AuthDevice,
  devices: readonly AuthDevice[],
): AuthDevice | undefined {
  if (device.platform !== EXTERNAL_PLATFORM || !device.loginDeviceId) {
    return undefined;
  }
  return devices.find((other) => other.id === device.loginDeviceId);
}

/**
 * The device list with each computer once, and under it the external devices
 * whose agents run there: their signed-in device's, else their own row.
 */
export function nestExternalDevices(
  devices: readonly AuthDevice[],
): { device: AuthDevice; externals: AuthDevice[] }[] {
  // Without agents to list it needs its own row, for its Disconnect.
  const hostOf = (device: AuthDevice) =>
    device.agents?.length ? loginDeviceOf(device, devices) : undefined;
  const rows = new Map<string, { device: AuthDevice; externals: AuthDevice[] }>(
    devices
      .filter((device) => !hostOf(device))
      .map((device) => [device.id, { device, externals: [] }]),
  );
  for (const device of devices) {
    const host = hostOf(device);
    if (host) {
      rows.get(host.id)?.externals.push(device);
    } else if (device.platform === EXTERNAL_PLATFORM) {
      rows.get(device.id)?.externals.push(device);
    }
  }
  return [...rows.values()];
}

/**
 * What an external agent runs on. `hermesPubkeys`: Hermes agents this app
 * connected, for connections older than the relay's runtimes.
 */
export function agentRuntime(
  agent: AuthDeviceAgent,
  hermesPubkeys: ReadonlySet<string>,
): string | null {
  const runtime = agent.runtime?.trim();
  if (runtime) return runtime;
  return hermesPubkeys.has(agent.pubkey.toLowerCase()) ? "Hermes" : null;
}

/** `remembered` without agents the relay no longer lists on their device. */
export function withoutDisconnected(
  remembered: readonly RememberedPairing[],
  devices: readonly AuthDevice[],
): RememberedPairing[] {
  const listed = new Map(
    devices
      .filter((device) => device.agents?.length)
      .map((device) => [
        device.id,
        new Set(device.agents?.map(({ pubkey }) => pubkey.toLowerCase())),
      ]),
  );
  return remembered.filter(
    (entry) => listed.get(entry.deviceId)?.has(entry.agentPubkey) ?? true,
  );
}

const HEX64 = /^[0-9a-f]{64}$/;

/** Remembered entries from stored JSON; anything malformed is skipped. */
export function parseRemembered(raw: string | null): RememberedPairing[] {
  if (!raw) return [];
  let value: unknown;
  try {
    value = JSON.parse(raw);
  } catch {
    return [];
  }
  if (!Array.isArray(value)) return [];
  return value.flatMap((item) => {
    if (typeof item !== "object" || item === null) return [];
    const { agentPubkey, deviceId, connectedAt, hermes } = item as Record<
      string,
      unknown
    >;
    if (typeof agentPubkey !== "string" || !HEX64.test(agentPubkey)) return [];
    if (typeof deviceId !== "string" || deviceId.length === 0) return [];
    return [
      {
        agentPubkey,
        deviceId,
        connectedAt: typeof connectedAt === "string" ? connectedAt : null,
        ...(hermes === true ? { hermes: true as const } : {}),
      },
    ];
  });
}

export type HermesRowState =
  | "connect"
  | "connected"
  | "otherAccount"
  | "blocked";

export type HermesRow = { profile: HermesProfile; state: HermesRowState };

/**
 * What each Hermes profile can do for this account. `connectedPubkeys`: this
 * account's connected agents, lowercase. A default profile connected before
 * pins existed counts as connected while this card's agent is connected
 * (`hermesDeviceConnected`; connecting another profile pins it). When a copied
 * profile shares a pin, only the first is connected. `otherAccount` can still
 * connect: that account may be long gone from this computer.
 */
export function hermesRows(
  profiles: readonly HermesProfile[],
  owner: string | null | undefined,
  connectedPubkeys: ReadonlySet<string>,
  hermesDeviceConnected: boolean,
): HermesRow[] {
  const account = owner?.toLowerCase();
  const seen = new Set<string>();
  return profiles.map((profile) => {
    const pin = profile.pairedAgent;
    const pinned = pin !== null && connectedPubkeys.has(pin) && !seen.has(pin);
    if (pinned) seen.add(pin);
    if (profile.blockedReason) return { profile, state: "blocked" };
    if (pinned) return { profile, state: "connected" };
    if (profile.owner && profile.owner !== account) {
      return { profile, state: "otherAccount" };
    }
    if (profile.isDefault && profile.owner && !pin && hermesDeviceConnected) {
      return { profile, state: "connected" };
    }
    return { profile, state: "connect" };
  });
}
