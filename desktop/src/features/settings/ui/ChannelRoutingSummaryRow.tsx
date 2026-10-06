import { ChevronRight } from "lucide-react";

import { useAppNavigation } from "@/app/navigation/useAppNavigation";
import { useManagedAgentsQuery } from "@/features/agents/hooks";
import { useChannelRoutingQuery } from "@/features/agents/channelRoutingHooks";
import {
  routingNameOf,
  routingSummaryText,
} from "@/features/agents/lib/channelRouting";
import { SettingsOptionRow } from "./SettingsOptionGroup";

/**
 * Settings › Agents › Conversations: a read-only summary of channel routing
 * with a link to the Agents page card, which is the only place it is edited
 * (mode, routing agent, and "Join new channels I create").
 */
export function ChannelRoutingSummaryRow() {
  const { goAgents } = useAppNavigation();
  const agents = useManagedAgentsQuery().data ?? [];
  const status = useChannelRoutingQuery().data;

  return (
    <SettingsOptionRow data-testid="settings-channel-routing-summary">
      <div className="min-w-0">
        <p className="font-medium text-foreground">
          {status
            ? routingSummaryText(status, routingNameOf(agents))
            : "Channel routing"}
        </p>
        <p
          className="mt-0.5 text-sm text-muted-foreground/70"
          data-settings-subcopy
        >
          Which agent picks up messages nobody @mentioned, and whether it joins
          channels you create.
        </p>
      </div>
      <button
        className="inline-flex shrink-0 items-center gap-1 text-sm font-medium text-primary hover:underline focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring"
        data-testid="settings-channel-routing-link"
        onClick={() => void goAgents()}
        type="button"
      >
        Change on the Agents page
        <ChevronRight aria-hidden="true" className="h-4 w-4" />
      </button>
    </SettingsOptionRow>
  );
}
