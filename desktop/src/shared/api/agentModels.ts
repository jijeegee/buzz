import type { AgentModelsResponse } from "@/shared/api/types";
import { invokeTauri } from "@/shared/api/tauri";

export type DiscoverAgentModelsInput = {
  acpCommand?: string;
  agentCommand: string;
  agentArgs?: string[];
  provider?: string;
  envVars?: Record<string, string>;
  /** Definition-level env from the harness definition (custom/preset). Merged below user envVars. */
  definitionEnv?: Record<string, string>;
  /** The saved agent being edited; a Hermes agent lists its own profile's models. */
  agentPubkey?: string;
  /** The model saved on the agent being edited, kept listed even if unavailable. */
  persistedModel?: string;
};

export async function discoverAgentModels(input: DiscoverAgentModelsInput) {
  return invokeTauri<AgentModelsResponse>("discover_agent_models", { input });
}
