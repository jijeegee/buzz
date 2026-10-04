import { EllipsisVertical } from "lucide-react";

import { startOnLaunchMenuState } from "@/features/agents/lib/startOnLaunchMenu";
import type { ManagedAgent } from "@/shared/api/types";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuTrigger,
} from "@/shared/ui/dropdown-menu";

export type AgentStartOnLaunchMenuItemProps = {
  /** The card's representative instance; `undefined` hides the entry. */
  agent: ManagedAgent | undefined;
  /** Only the start-on-launch mutation's own pending state, never the page-wide one. */
  isPending: boolean;
  onToggleStartOnLaunch: (agent: ManagedAgent, next: boolean) => void;
};

/**
 * The one user-reachable "Start on launch" control: a checkbox entry in the
 * Agents page card menu. The checkbox item owns the accessible name
 * (`role="menuitemcheckbox"` + `aria-checked`); nothing else labels it.
 */
export function AgentStartOnLaunchMenuItem({
  agent,
  isPending,
  onToggleStartOnLaunch,
}: AgentStartOnLaunchMenuItemProps) {
  const state = startOnLaunchMenuState(agent);
  if (state === "hidden" || agent === undefined) return null;

  const providerManaged = state === "provider-managed";
  return (
    <DropdownMenuCheckboxItem
      checked={agent.startOnAppLaunch}
      data-testid={`agent-start-on-launch-${agent.pubkey}`}
      disabled={providerManaged || isPending}
      onCheckedChange={(next) => onToggleStartOnLaunch(agent, next)}
      onSelect={(event) => event.preventDefault()}
    >
      <span className="flex min-w-0 flex-col">
        <span>Start on launch</span>
        {providerManaged ? (
          <span className="text-xs text-muted-foreground">
            Managed by provider
          </span>
        ) : null}
      </span>
    </DropdownMenuCheckboxItem>
  );
}

/**
 * Actions menu for a definition-less instance card, which has no persona menu
 * to host the entry.
 */
export function StandaloneAgentActionsMenu({
  agent,
  isPending,
  onToggleStartOnLaunch,
}: AgentStartOnLaunchMenuItemProps & { agent: ManagedAgent }) {
  return (
    <DropdownMenu modal={false}>
      <DropdownMenuTrigger asChild>
        <button
          aria-label={`Open actions for ${agent.name}`}
          className="flex h-7 w-7 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
          type="button"
        >
          <EllipsisVertical className="h-4 w-4" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent
        align="end"
        onCloseAutoFocus={(event) => event.preventDefault()}
      >
        <AgentStartOnLaunchMenuItem
          agent={agent}
          isPending={isPending}
          onToggleStartOnLaunch={onToggleStartOnLaunch}
        />
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
