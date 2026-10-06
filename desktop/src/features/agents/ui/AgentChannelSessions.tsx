import * as React from "react";
import { useQueryClient } from "@tanstack/react-query";
import { ArrowUpRight, ChevronDown, ChevronRight } from "lucide-react";

import {
  type AgentContextUsage,
  type ChannelSessionGroup,
  groupContextUsagesByChannel,
  useAgentContextUsages,
} from "@/features/agents/agentContextUsageStore";
import {
  formatContextUpdatedAgo,
  formatContextUsageLabel,
} from "@/features/agents/lib/contextGauge";
import { channelMessagesKey } from "@/features/messages/lib/messageQueryKeys";
import type { RelayEvent } from "@/shared/api/types";
import { cn } from "@/shared/lib/cn";
import { useNow } from "@/shared/lib/useNow";
import { Button } from "@/shared/ui/button";
import {
  CompactSessionDialog,
  ContextGaugeButton,
  ContextGaugeDial,
} from "./AgentContextGauge";

/** Thread sessions shown before the rest collapse behind "Show more". */
export const VISIBLE_THREAD_SESSIONS = 10;
const SNIPPET_MAX_CHARS = 60;

const EMPTY_GROUP: ChannelSessionGroup = { conversation: null, threads: [] };

/** Per-channel session readings for one agent, re-derived on store change. */
export function useAgentChannelSessionGroups(
  agentPubkey: string | null | undefined,
): Map<string, ChannelSessionGroup> {
  const usages = useAgentContextUsages(agentPubkey);
  return React.useMemo(() => groupContextUsagesByChannel(usages), [usages]);
}

function snippetFromContent(content: string): string | null {
  const collapsed = content.replace(/\s+/g, " ").trim();
  if (!collapsed) return null;
  return collapsed.length > SNIPPET_MAX_CHARS
    ? `${collapsed.slice(0, SNIPPET_MAX_CHARS - 1)}…`
    : collapsed;
}

/**
 * Root-message snippet when the channel timeline is already cached; otherwise
 * a short event id. Never fetches.
 */
function useThreadLabel(channelId: string, rootEventId: string): string {
  const queryClient = useQueryClient();
  const cached = queryClient.getQueryData<unknown>(
    channelMessagesKey(channelId),
  );
  const root = Array.isArray(cached)
    ? (cached as RelayEvent[]).find((event) => event?.id === rootEventId)
    : undefined;
  const snippet =
    root && typeof root.content === "string"
      ? snippetFromContent(root.content)
      : null;
  return snippet ?? `Thread ${rootEventId.slice(0, 8)}`;
}

function ThreadSessionRow({
  agentName,
  now,
  reading,
}: {
  agentName: string;
  now: number;
  reading: AgentContextUsage;
}) {
  const [open, setOpen] = React.useState(false);
  const threadRoot = reading.threadRootEventId ?? "";
  const label = useThreadLabel(reading.channelId, threadRoot);
  const usageLabel = formatContextUsageLabel(reading.used, reading.size);
  return (
    <li
      className="flex items-center gap-3 py-2 pr-4 pl-8 text-sm"
      data-testid="agent-thread-session-row"
    >
      <ContextGaugeDial size={16} reading={reading} />
      <div className="min-w-0 flex-1">
        <p className="truncate" title={label}>
          {label}
        </p>
        <p className="truncate text-xs text-muted-foreground">
          {usageLabel} · {formatContextUpdatedAgo(reading.updatedAt, now)}
        </p>
      </div>
      <Button
        aria-label={`Compact session: ${label}`}
        onClick={() => setOpen(true)}
        size="sm"
        type="button"
        variant="outline"
      >
        Compact
      </Button>
      {open ? (
        <CompactSessionDialog
          agentName={agentName}
          onOpenChange={setOpen}
          open={open}
          reading={reading}
        />
      ) : null}
    </li>
  );
}

function ThreadSessionList({
  agentName,
  threads,
}: {
  agentName: string;
  threads: readonly AgentContextUsage[];
}) {
  const [showAll, setShowAll] = React.useState(false);
  const now = useNow(60_000);
  const visible = showAll ? threads : threads.slice(0, VISIBLE_THREAD_SESSIONS);
  const hiddenCount = threads.length - visible.length;
  return (
    <div data-testid="agent-thread-session-list">
      <ul className="divide-y divide-border/40">
        {visible.map((reading) => (
          <ThreadSessionRow
            agentName={agentName}
            key={reading.threadRootEventId}
            now={now}
            reading={reading}
          />
        ))}
      </ul>
      {hiddenCount > 0 ? (
        <div className="py-2 pl-8">
          <Button
            onClick={() => setShowAll(true)}
            size="sm"
            type="button"
            variant="ghost"
          >
            Show {hiddenCount} more
          </Button>
        </div>
      ) : null}
    </div>
  );
}

/**
 * One channel row in the agent's Channels section, with the agent's
 * whole-conversation gauge and an expandable list of its thread sessions.
 */
export function AgentChannelSessionsRow({
  agentName,
  channel,
  group = EMPTY_GROUP,
  onOpenChannel,
}: {
  agentName: string;
  channel: { id: string; name: string };
  group?: ChannelSessionGroup;
  onOpenChannel: (channelId: string) => void;
}) {
  const [expanded, setExpanded] = React.useState(false);
  const threadCount = group.threads.length;
  const hasSessions = group.conversation !== null || threadCount > 0;
  const listId = React.useId();
  return (
    <div>
      <div
        className={cn(
          "flex min-h-16 items-center gap-1",
          hasSessions && "pr-2",
        )}
      >
        <button
          aria-label={`Open #${channel.name}`}
          className="group flex min-w-0 flex-1 items-center gap-3 self-stretch px-4 py-3 text-left text-sm font-medium text-foreground transition-colors hover:bg-muted/40"
          data-testid={`user-profile-channel-link-${channel.name}`}
          onClick={() => onOpenChannel(channel.id)}
          type="button"
        >
          <span className="min-w-0 flex-1 truncate">#{channel.name}</span>
          <ArrowUpRight
            aria-hidden="true"
            className="h-4 w-4 shrink-0 text-muted-foreground transition-colors group-hover:text-foreground"
          />
        </button>
        {group.conversation ? (
          <ContextGaugeButton
            agentName={agentName}
            className="h-7 w-7"
            dialSize={18}
            reading={group.conversation}
          />
        ) : null}
        {threadCount > 0 ? (
          <Button
            aria-controls={listId}
            aria-expanded={expanded}
            className="gap-1 text-xs text-muted-foreground"
            data-testid={`user-profile-channel-sessions-toggle-${channel.name}`}
            onClick={() => setExpanded((value) => !value)}
            size="sm"
            type="button"
            variant="ghost"
          >
            {expanded ? (
              <ChevronDown aria-hidden="true" className="h-4 w-4" />
            ) : (
              <ChevronRight aria-hidden="true" className="h-4 w-4" />
            )}
            {threadCount === 1 ? "1 thread" : `${threadCount} threads`}
          </Button>
        ) : null}
      </div>
      {threadCount > 0 ? (
        <div hidden={!expanded} id={listId}>
          {expanded ? (
            <ThreadSessionList agentName={agentName} threads={group.threads} />
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
