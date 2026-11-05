import { invokeTauri } from "@/shared/api/tauri";
import type {
  GlobalAgentConfig,
  GlobalAgentConfigSaveResult,
} from "@/shared/api/types";

/**
 * Read the active account's agent configuration defaults.
 *
 * Returns an empty default if the file has not been written yet.
 */
export async function getGlobalAgentConfig(): Promise<GlobalAgentConfig> {
  return invokeTauri<GlobalAgentConfig>("get_global_agent_config");
}

/** The account a settings save is for; Rust refuses it once that account is no longer active. */
export function requireOwner(owner: string | null | undefined) {
  if (!owner) throw new Error("Your account isn't ready yet. Try again.");
  return owner;
}

/**
 * Validate and persist an account's agent configuration. `owner` is the
 * account the screen shows; a save from before an account switch is refused.
 *
 * The backend strips empty env values (empty = "inherit"), validates key
 * shape and reserved-key rules, restarts running local agents whose effective
 * env changed, and returns the saved config with a restart count.
 *
 * Throws a string error message on validation failure.
 */
export async function setGlobalAgentConfig(
  config: GlobalAgentConfig,
  owner: string | null | undefined,
): Promise<GlobalAgentConfigSaveResult> {
  return invokeTauri<GlobalAgentConfigSaveResult>("set_global_agent_config", {
    config,
    owner: requireOwner(owner),
  });
}

/** Whether `pubkey` signed in here since its agents were last checked. */
export function getAgentSetupPending(pubkey: string): Promise<boolean> {
  return invokeTauri<boolean>("get_agent_setup_pending", { pubkey });
}

export function finishAgentSetupPending(pubkey: string): Promise<void> {
  return invokeTauri<void>("finish_agent_setup_pending", { pubkey });
}
