import {
  CircleCheck,
  CircleDashed,
  RefreshCw,
  TriangleAlert,
} from "lucide-react";

import type {
  RoutingRestartAffordance,
  RoutingStatusLine,
} from "@/features/agents/lib/channelRouting";
import { cn } from "@/shared/lib/cn";
import { Button } from "@/shared/ui/button";

const TONE_ICON = {
  on: CircleCheck,
  off: CircleDashed,
  switching: RefreshCw,
  attention: TriangleAlert,
} as const;

/**
 * The card's one status line (the applied state from Rust's transition plan)
 * and, while switching, a "Restart <name> now" button per agent whose running
 * role is out of date. The button reuses the page's ordinary restart; it is
 * disabled with its reason while the plan holds the agent or it is mid-turn.
 */
export function ChannelRoutingStatusLine({
  affordances,
  isRestarting,
  line,
  onRestart,
}: {
  affordances: readonly RoutingRestartAffordance[];
  isRestarting: (pubkey: string) => boolean;
  line: RoutingStatusLine;
  onRestart: (pubkey: string) => void;
}) {
  const Icon = TONE_ICON[line.tone];
  return (
    <div className="space-y-2 border-t border-border/60 pt-3">
      <p
        className={cn(
          "flex items-start gap-2 text-sm",
          line.tone === "on" && "text-foreground",
          line.tone === "off" && "text-muted-foreground",
          line.tone === "switching" && "text-foreground",
          line.tone === "attention" && "text-amber-700 dark:text-amber-400",
        )}
        data-testid="agents-channel-routing-status"
        data-tone={line.tone}
        role="status"
      >
        <Icon aria-hidden="true" className="mt-0.5 h-4 w-4 shrink-0" />
        <span className="min-w-0">{line.text}</span>
      </p>
      {affordances.map((affordance) => (
        <div
          className="ml-6 flex flex-wrap items-center gap-x-3 gap-y-1"
          data-testid="agents-channel-routing-restart"
          key={affordance.pubkey}
        >
          {affordance.label ? (
            <Button
              disabled={affordance.disabled || isRestarting(affordance.pubkey)}
              onClick={() => onRestart(affordance.pubkey)}
              size="sm"
              type="button"
              variant="outline"
            >
              <RefreshCw aria-hidden="true" />
              {affordance.label}
            </Button>
          ) : null}
          {affordance.reason ? (
            <p className="text-xs text-muted-foreground">{affordance.reason}</p>
          ) : null}
        </div>
      ))}
    </div>
  );
}
