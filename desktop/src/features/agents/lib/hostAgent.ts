import { discoverAcpRuntimes } from "@/shared/api/tauriAcpDiscovery";
import { getGlobalAgentConfig } from "@/shared/api/tauriGlobalAgentConfig";
import { listPersonas, setPersonaActive } from "@/shared/api/tauriPersonas";
import type { AcpRuntime, CreateManagedAgentInput } from "@/shared/api/types";
import {
  buildInstanceInputForDefinition,
  resolveStartRuntimeForDefinition,
} from "./instanceInputForDefinition";

/** The Rust-seeded built-in persona that routes as the Host. */
export const HOST_PERSONA_ID = "builtin:host";

/**
 * The create input for a new instance of the built-in Host persona, through
 * the same definition→instance mapping every other surface uses. Re-adds the
 * persona to My Agents first when the user removed it, since creating an
 * instance of an inactive definition is refused.
 */
export async function buildHostAgentCreateInput(): Promise<CreateManagedAgentInput> {
  let persona = (await listPersonas()).find(
    (candidate) => candidate.id === HOST_PERSONA_ID,
  );
  if (!persona) {
    throw new Error("The built-in Host agent is missing.");
  }
  if (!persona.isActive) {
    persona = await setPersonaActive(HOST_PERSONA_ID, true);
  }
  const [catalog, globalConfig] = await Promise.all([
    discoverAcpRuntimes(),
    getGlobalAgentConfig(),
  ]);
  const runtimes = catalog.filter(
    (runtime): runtime is AcpRuntime => runtime.availability === "available",
  );
  const { runtime } = resolveStartRuntimeForDefinition(
    persona,
    runtimes,
    globalConfig.preferred_runtime,
  );
  return buildInstanceInputForDefinition(persona, runtime);
}
