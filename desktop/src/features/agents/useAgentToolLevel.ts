import { useQuery } from "@tanstack/react-query";

import {
  getAgentToolLevels,
  type ToolLevel,
} from "@/shared/api/tauriAgentToolLevels";

export function agentToolLevelQueryKey(pubkey: string) {
  return ["agent-tool-level", pubkey.toLowerCase()] as const;
}

/** Whether owner tools are on for one agent: `true`/`false`, or `null`
 * while unknown (loading, failed, or not registered). Never set reads off. */
export function useAgentToolLevelQuery(
  pubkey: string,
  options?: { enabled?: boolean },
) {
  return useQuery({
    queryKey: agentToolLevelQueryKey(pubkey),
    queryFn: async (): Promise<ToolLevel | null> => {
      const key = pubkey.toLowerCase();
      const rows = await getAgentToolLevels([key]);
      const row = rows.find((candidate) => candidate.pubkey === key);
      if (!row) return null;
      return row.level ?? "limited";
    },
    enabled: options?.enabled ?? true,
    gcTime: 0,
    retry: false,
  });
}
