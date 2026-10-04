/**
 * React hook: load the global agent configuration defaults.
 *
 * Backed by TanStack Query with a stable query key so the config is fetched
 * once per QueryClient lifetime and shared across all callers — dialogs always
 * receive the already-populated value on first render, eliminating the
 * per-mount IPC race that caused required-env-key rows to be missing on open.
 *
 * On fetch error the query falls back to EMPTY_CONFIG (safe — the absence of
 * a global config is never an error state for callers).
 */
import { useQuery } from "@tanstack/react-query";

import { getGlobalAgentConfig } from "@/shared/api/tauriGlobalAgentConfig";
import type { GlobalAgentConfig } from "@/shared/api/types";

const EMPTY_CONFIG: GlobalAgentConfig = {
  env_vars: {},
  provider: null,
  model: null,
  preferred_runtime: null,
  effort_level: null,
};

export const globalAgentConfigQueryKey = ["globalAgentConfig"] as const;

export function useGlobalAgentConfig(): {
  globalConfig: GlobalAgentConfig;
  isLoading: boolean;
  /**
   * True only once the backend's real record is in hand (fetched, not the
   * `EMPTY_CONFIG` placeholder and not the post-error fallback). Surfaces
   * that spread `globalConfig` back into `set_global_agent_config` must
   * refuse to write until this is true, or they wipe every key and default
   * that was not loaded yet.
   */
  isReady: boolean;
  isError: boolean;
} {
  const { data, isError, isPending, isPlaceholderData, status } = useQuery({
    queryKey: globalAgentConfigQueryKey,
    queryFn: getGlobalAgentConfig,
    // Config is only mutated via setGlobalAgentConfig — treat as stable until
    // explicitly invalidated by AgentDefaultsSettingsCard after a save.
    staleTime: Number.POSITIVE_INFINITY,
    // Never show a stale empty flash while a background refetch runs.
    placeholderData: EMPTY_CONFIG,
  });

  return {
    globalConfig: data ?? EMPTY_CONFIG,
    isLoading: isPending,
    isReady: status === "success" && !isPlaceholderData,
    isError,
  };
}
