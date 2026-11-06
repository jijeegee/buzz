import { listen } from "@tauri-apps/api/event";
import { useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import { toast } from "sonner";

import {
  managedAgentsQueryKey,
  personasQueryKey,
  teamsQueryKey,
} from "@/features/agents/hooks";
import { managedAgentRuntimesQueryKey } from "@/features/agents/managedAgentRuntimeHooks";
import { teamAutoRetractedNotice } from "@/features/agents/ui/teamLibraryCopy";
import { t } from "@/shared/i18n";

export const LOCAL_AGENT_DATA_QUERY_KEYS = [
  personasQueryKey,
  teamsQueryKey,
  managedAgentsQueryKey,
] as const;

// Trailing-coalesce local agent-store bursts into one cache refresh. The relay
// directory is deliberately excluded: local persona/team/agent reconciliation
// cannot change remote directory records, and rebuilding that directory is a
// relay-wide operation. Remote data keeps its focused poll and is revalidated
// directly before an agent mention is sent.
const COALESCE_MS = 200;

/** One notice (Q1a) for every agent whose owner tools were turned on by
 * default this run; later defaults join the same toast. */
const OWNER_TOOLS_NOTICE_ID = "owner-tools-defaulted";

export function useAgentsDataRefresh(): void {
  const queryClient = useQueryClient();

  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    const ownerToolsNames: string[] = [];

    const unlistenRuntime = listen("managed-agent-runtime-status", () => {
      void queryClient.invalidateQueries({
        queryKey: managedAgentRuntimesQueryKey,
      });
      void queryClient.invalidateQueries({ queryKey: managedAgentsQueryKey });
    });

    const unlisten = listen("agents-data-changed", () => {
      if (timer !== undefined) clearTimeout(timer);
      timer = setTimeout(() => {
        for (const queryKey of LOCAL_AGENT_DATA_QUERY_KEYS) {
          void queryClient.invalidateQueries({ queryKey });
        }
      }, COALESCE_MS);
    });

    // Typed notice for automatic team catalog retractions (I4): the boot
    // reconcile detected a shared team that can no longer be projected and
    // tombstoned it. Show a toast so the owner is not left wondering why
    // their share toggle changed.
    const unlistenRetracted = listen<{
      teamName: string;
      reason: string;
    }>("team-catalog-auto-retracted", (event) => {
      toast.warning(
        teamAutoRetractedNotice(event.payload.teamName, event.payload.reason),
      );
      // Invalidate team queries so the share toggle reflects the retraction.
      void queryClient.invalidateQueries({ queryKey: teamsQueryKey });
    });

    const unlistenOwnerTools = listen<{ names: string[] }>(
      "owner-tools-defaulted",
      (event) => {
        for (const name of event.payload.names) {
          if (!ownerToolsNames.includes(name)) ownerToolsNames.push(name);
        }
        toast(
          t("agents.ownerTools.defaultedNotice", {
            names: ownerToolsNames.join(", "),
          }),
          {
            id: OWNER_TOOLS_NOTICE_ID,
            description: t("agents.ownerTools.defaultedNoticeBody"),
            duration: 15000,
          },
        );
      },
    );

    return () => {
      if (timer !== undefined) clearTimeout(timer);
      void unlistenOwnerTools.then((fn) => fn());
      void unlisten.then((fn) => fn());
      void unlistenRuntime.then((fn) => fn());
      void unlistenRetracted.then((fn) => fn());
    };
  }, [queryClient]);
}
