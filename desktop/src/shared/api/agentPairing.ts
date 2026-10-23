import { invokeTauri } from "@/shared/api/tauri";

/**
 * External agent pairing (device-bound keys, PR C2). The app shows a one-use,
 * ten-minute `kuzz pair <code>` command; the first valid claim on another
 * computer connects that agent at once. Disconnecting signs that computer
 * out and forgets its Hermes key (`disconnectExternalDevice`). Only for
 * accounts signed in with Google.
 */

/** A new code, as `kuzz pair <code>`. */
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

export type HermesAgent = {
  agentPubkey: string;
  deviceId: string | null;
};

/**
 * Whether Hermes is on this computer, whether this app can connect it (the
 * `kuzz pair --hermes` installer is here too), and the Hermes agents paired
 * here.
 */
export type HermesStatus = {
  found: boolean;
  canConnect: boolean;
  agents: HermesAgent[];
};

export type HermesConnected = {
  agentPubkey: string;
  deviceId: string;
  deviceName: string | null;
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

export function connectHermes(): Promise<HermesConnected> {
  return invokeTauri<HermesConnected>("connect_hermes");
}

/** Sign a paired computer out and forget its Hermes key, if any. */
export function disconnectExternalDevice(deviceId: string): Promise<void> {
  return invokeTauri<void>("disconnect_external_device", { deviceId });
}
