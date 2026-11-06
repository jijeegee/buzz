import { invokeTauri } from "@/shared/api/tauri";
import type { RespondToMode } from "@/shared/api/types";

/**
 * External agent pairing (device-bound keys, PR C2). The app shows a one-use,
 * ten-minute `kuzz pair --relay <url> <code>` command; the first valid claim on another
 * computer connects that agent at once. Disconnecting signs that computer
 * out (`disconnectExternalDevice`). Only for accounts signed in with Google.
 */

/** A new code; `command` is shown exactly as the relay gave it. */
export type PairingStart = {
  pairingId: string;
  command: string;
  expiresAt: string;
  expiresIn: number;
};

export type PairingState = "waiting" | "connected" | "expired";

/** One pairing of the last day (`GET /auth/pairing`). */
export type AgentPairing = {
  id: string;
  state: PairingState;
  agentPubkey: string | null;
  deviceName: string | null;
  deviceId: string | null;
  createdAt: string;
  claimedAt: string | null;
  expiresAt: string;
};

/**
 * Whether Hermes is on this computer, and whether this app can connect it
 * (its bundled `kuzz` offers `pair --hermes` and has the plugin files).
 */
export type HermesStatus = {
  found: boolean;
  canConnect: boolean;
  profiles: HermesProfile[];
};

/** What Hermes's state file says about a gateway. */
export type HermesGateway = {
  running: boolean;
  /** Hermes's state of the Kuzz platform. `null`: not loaded. */
  kuzz: string | null;
  /** Hermes's words on what is wrong with the Kuzz platform. */
  error: string | null;
  /** It has work running. */
  busy: boolean;
};

/** A Hermes profile a person can connect (`name` is `default` or its folder). */
export type HermesProfile = {
  name: string;
  displayName: string;
  isDefault: boolean;
  pairedAgent: string | null;
  owner: string | null;
  blockedReason: string | null;
  /** Its gateway, from Hermes's state file. `null`: nothing is known. */
  gateway: HermesGateway | null;
  /** This app is getting its gateway to load the connector. */
  settling: boolean;
};

/**
 * One run of the bundled `kuzz pair --hermes` on a new code. `finished`:
 * the CLI ended without an error. The pairing list says whether it
 * connected.
 */
export type HermesPairingRun = {
  pairingId: string;
  finished: boolean;
};

export function startAgentPairing(): Promise<PairingStart> {
  return invokeTauri<PairingStart>("start_agent_pairing");
}

export function listAgentPairings(): Promise<AgentPairing[]> {
  return invokeTauri<AgentPairing[]>("list_agent_pairings");
}

export function getHermesStatus(): Promise<HermesStatus> {
  return invokeTauri<HermesStatus>("get_hermes_status");
}

/**
 * `sameAccount`: the profile was connected for this account, so its agent is
 * paired again rather than a new one.
 */
export function connectHermes(
  profile: string,
  sameAccount: boolean,
): Promise<HermesPairingRun> {
  return invokeTauri<HermesPairingRun>("connect_hermes", {
    profile,
    sameAccount,
  });
}

/** Have the gateway of a connected profile load the connector. */
export function restartHermes(profile: string): Promise<void> {
  return invokeTauri<void>("restart_hermes", { profile });
}

/** Who a paired agent answers. */
export type ExternalAgentAccess = {
  respondTo: RespondToMode;
  respondToAllowlist: string[];
};

export function getExternalAgentAccess(
  pubkey: string,
): Promise<ExternalAgentAccess> {
  return invokeTauri<ExternalAgentAccess>("get_external_agent_access", {
    pubkey,
  });
}

/** Resolves once the relay accepted the owner-signed setting. `toolLevel`:
 * the owner turned owner tools on or off in this save. */
export function setExternalAgentAccess(
  pubkey: string,
  name: string,
  respondTo: RespondToMode,
  allowlist: string[],
  toolLevel?: "full" | "limited",
): Promise<void> {
  return invokeTauri<void>("set_external_agent_access", {
    pubkey,
    name,
    respondTo,
    allowlist,
    ...(toolLevel ? { toolLevel } : {}),
  });
}

/** Set a paired agent's name, picture and description, from any device. */
export function setExternalAgentProfile(
  pubkey: string,
  profile: {
    displayName: string;
    avatarUrl: string | null;
    about: string | null;
  },
): Promise<void> {
  return invokeTauri<void>("set_external_agent_profile", {
    pubkey,
    ...profile,
  });
}

/** Sign a paired computer out. */
export function disconnectExternalDevice(deviceId: string): Promise<void> {
  return invokeTauri<void>("disconnect_external_device", { deviceId });
}

/** Take one agent off a paired computer. Its last agent takes the computer. */
export function disconnectExternalAgent(
  deviceId: string,
  pubkey: string,
): Promise<void> {
  return invokeTauri<void>("disconnect_external_agent", { deviceId, pubkey });
}
