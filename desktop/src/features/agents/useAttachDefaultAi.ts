import * as React from "react";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

import { attachManagedAgentToChannel } from "@/features/agents/channelAgents";
import { channelRoutingQueryKey } from "@/features/agents/channelRoutingHooks";
import { managedAgentsQueryKey } from "@/features/agents/hooks";
import {
  findDefaultAi,
  routingJoinsNewChannels,
} from "@/features/agents/lib/defaultAi";
import { useDefaultAi } from "@/features/agents/useDefaultAi";
import { getChannelRouting } from "@/shared/api/tauriChannelRouting";
import type { ManagedAgent } from "@/shared/api/types";

/**
 * Adds the default AI to a channel you just created, as a bot, starting it
 * when it is not already running. A no-op when no agent is starred or the
 * saved channel routing mode is not Host or Lead (read fresh at call time).
 *
 * `attachDefaultAi` never rejects: a failed membership write or start is
 * reported with a warning toast so callers can fire-and-forget it after
 * navigation. Callers that also add template agents must sequence the two
 * (await this first) — concurrent writes to the replaceable membership event
 * are last-write-wins.
 */
export function useAttachDefaultAi() {
  const queryClient = useQueryClient();
  const { defaultAi } = useDefaultAi();

  const attachDefaultAi = React.useCallback(
    async (channelId: string) => {
      // Resolve the star from the cache at call time only — never from the
      // render closure. A stale closure could attach a record that has since
      // been un-starred or deleted as a ghost member, and the cache also
      // carries the fresh `status` the attach uses to decide on a start.
      const cached = queryClient.getQueryData<ManagedAgent[]>(
        managedAgentsQueryKey,
      );
      const agent = cached ? findDefaultAi(cached) : null;
      if (!agent) return;
      // The mode is read at call time too: a form opened under Host must not
      // join after the user switched routing Off in the meantime.
      let joins: boolean;
      try {
        const routing = await queryClient.fetchQuery({
          queryKey: channelRoutingQueryKey,
          queryFn: getChannelRouting,
        });
        joins = routingJoinsNewChannels(routing.mode);
      } catch (error) {
        toast.warning(`${agent.name} was not added to this channel`, {
          description:
            error instanceof Error
              ? error.message
              : "Couldn't read channel routing.",
        });
        return;
      }
      if (!joins) return;

      try {
        await attachManagedAgentToChannel(channelId, {
          agent,
          role: "bot",
          ensureRunning: true,
        });
      } catch (error) {
        toast.warning(`${agent.name} could not be added to this channel`, {
          description:
            error instanceof Error ? error.message : "Failed to add agent.",
        });
      } finally {
        // Same keys `useApplyTemplate.applyAgents` refreshes after adding
        // template agents: the roster, the managed list (status), relay agents.
        await Promise.all([
          queryClient.invalidateQueries({
            queryKey: ["channels", channelId, "members"],
          }),
          queryClient.invalidateQueries({ queryKey: managedAgentsQueryKey }),
          queryClient.invalidateQueries({ queryKey: ["relay-agents"] }),
        ]);
      }
    },
    [queryClient],
  );

  return { attachDefaultAi, hasDefaultAi: defaultAi !== null };
}
