import type { ManagedAgent } from "@/shared/api/types";

/**
 * How the "Start on launch" entry of an agent card's actions menu renders.
 *
 * - `"hidden"`: no instance record exists (a definition-only persona or a
 *   key-less definition), so there is nothing to persist the flag on. Once
 *   the card's Start creates an instance (created with
 *   `startOnAppLaunch: true`), the next menu open shows the entry.
 * - `"provider-managed"`: the instance runs on a provider backend. Rust
 *   forces `start_on_app_launch = false` at create time for those, so the
 *   entry stays visible but disabled with a "Managed by provider" hint.
 * - `"toggle"`: a local instance — the entry is a live checkbox item.
 */
export type StartOnLaunchMenuState = "toggle" | "provider-managed" | "hidden";

export function startOnLaunchMenuState(
  agent: Pick<ManagedAgent, "backend"> | undefined,
): StartOnLaunchMenuState {
  if (agent === undefined) return "hidden";
  return agent.backend.type === "local" ? "toggle" : "provider-managed";
}
