import * as React from "react";

import { useActiveAgentTurns } from "@/features/agents/activeAgentTurnsStore";
import {
  type AgentContextUsage,
  useResolvedAgentContextUsage,
} from "@/features/agents/agentContextUsageStore";
import {
  type ContextGaugeStage,
  type ContextGaugeThresholds,
  contextFillFraction,
  contextGaugeStage,
  contextGaugeWedge,
  DEFAULT_CONTEXT_GAUGE_THRESHOLDS,
  formatContextUpdatedAgo,
  formatContextUsageLabel,
} from "@/features/agents/lib/contextGauge";
import { useContextGaugeEnabled } from "@/features/agents/lib/contextGaugePreference";
import {
  type ContextReading,
  compactDialogView,
  compactUnavailableReason,
} from "@/features/agents/lib/contextUsageQuery";
import { cn } from "@/shared/lib/cn";
import { useNow } from "@/shared/lib/useNow";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/shared/ui/alert-dialog";
import { Button } from "@/shared/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/shared/ui/tooltip";
import {
  runCompactSession,
  useCompactSessionPending,
} from "./runCompactSession";
import { useContextUsageQuery } from "./useContextUsageQuery";

const STAGE_FILL_CLASS: Record<ContextGaugeStage, string> = {
  normal: "fill-muted-foreground/35",
  warning: "fill-yellow-500",
  critical: "fill-red-500",
};

const VIEWBOX = 20;
const CENTER = VIEWBOX / 2;
const RADIUS = 8.5;

/** Clock-dial glyph: dashed empty ring, wedge filled clockwise from 12. */
export function ContextGaugeDial({
  className,
  reading,
  size,
  thresholds = DEFAULT_CONTEXT_GAUGE_THRESHOLDS,
}: {
  className?: string;
  reading: { used: number; size: number };
  size: number;
  thresholds?: ContextGaugeThresholds;
}) {
  const fraction = contextFillFraction(reading.used, reading.size);
  const stage = contextGaugeStage(fraction, thresholds);
  const fillClass = STAGE_FILL_CLASS[stage];
  const wedge = contextGaugeWedge(fraction, CENTER, RADIUS);
  return (
    <svg
      aria-hidden="true"
      className={cn("shrink-0", className)}
      data-stage={stage}
      height={size}
      viewBox={`0 0 ${VIEWBOX} ${VIEWBOX}`}
      width={size}
    >
      <circle
        className="stroke-muted-foreground/70"
        cx={CENTER}
        cy={CENTER}
        fill="none"
        r={RADIUS}
        strokeDasharray="2 1.6"
        strokeWidth={1.2}
      />
      {wedge.kind === "full" ? (
        <circle className={fillClass} cx={CENTER} cy={CENTER} r={RADIUS} />
      ) : wedge.kind === "wedge" ? (
        <path className={fillClass} d={wedge.path} />
      ) : null}
    </svg>
  );
}

function scopeDescription(reading: AgentContextUsage): string {
  return reading.threadRootEventId
    ? `Thread ${reading.threadRootEventId.slice(0, 8)}`
    : "Whole conversation";
}

/**
 * Confirmation for compacting one session. On open it asks the harness for
 * the session's current reading and shows it (or the cached reading, marked
 * possibly stale, when the harness doesn't answer). Compact sends the shown
 * reading as the expectation; a `stale` refusal replaces it with the
 * harness's newer reading and keeps the dialog open.
 */
export function CompactSessionDialog({
  agentName,
  onOpenChange,
  open,
  reading,
}: {
  agentName: string;
  onOpenChange: (open: boolean) => void;
  open: boolean;
  reading: AgentContextUsage;
}) {
  const activeTurns = useActiveAgentTurns(reading.agentPubkey);
  const hasActiveTurn = activeTurns.some(
    (turn) => turn.channelId === reading.channelId,
  );
  const pending = useCompactSessionPending(reading);
  const query = useContextUsageQuery(reading);
  const [changed, setChanged] = React.useState<ContextReading | null>(null);
  const [submitting, setSubmitting] = React.useState(false);
  const now = useNow(30_000);
  const view = compactDialogView({ cached: reading, query, changed, now });
  const unavailableReason = compactUnavailableReason({
    compactSupported: view.reading.compactSupported,
    hasActiveTurn,
    noSession: view.noSession,
    pending: pending || submitting,
  });

  const compact = () => {
    setSubmitting(true);
    void runCompactSession(
      {
        agentPubkey: reading.agentPubkey,
        agentName,
        channelId: reading.channelId,
        threadRootEventId: reading.threadRootEventId,
      },
      {
        expected: {
          sessionId: view.reading.sessionId,
          used: view.reading.used,
        },
        onStarted: () => onOpenChange(false),
        onStale: (current) => {
          // Without a reading the old one is not trustworthy either: re-check.
          if (current) setChanged(current);
        },
      },
    ).then((outcome) => {
      setSubmitting(false);
      if (outcome !== "stale") onOpenChange(false);
    });
  };

  return (
    <AlertDialog onOpenChange={onOpenChange} open={open}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Compact {agentName}'s session?</AlertDialogTitle>
          <AlertDialogDescription>
            Compacting summarizes the earlier conversation in this session so
            the agent has more room to keep working.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <div
          className="flex items-center gap-3 rounded-lg border border-border/60 px-3 py-2 text-sm"
          data-freshness={view.freshness}
          data-testid="compact-session-usage"
        >
          <ContextGaugeDial size={28} reading={view.reading} />
          <div className="min-w-0">
            <p className="font-medium">
              {formatContextUsageLabel(view.reading.used, view.reading.size)}
            </p>
            <p className="text-xs text-muted-foreground">
              {scopeDescription(reading)} · Updated{" "}
              {formatContextUpdatedAgo(view.updatedAt, now)}
            </p>
          </div>
        </div>
        {view.note ? (
          <p
            aria-live="polite"
            className="text-sm text-muted-foreground"
            data-testid="compact-session-freshness"
          >
            {view.note}
          </p>
        ) : null}
        {unavailableReason ? (
          <p
            className="text-sm text-muted-foreground"
            data-testid="compact-session-unavailable"
          >
            {unavailableReason}
          </p>
        ) : null}
        <AlertDialogFooter>
          <AlertDialogCancel asChild>
            <Button type="button" variant="outline">
              Cancel
            </Button>
          </AlertDialogCancel>
          <Button
            disabled={view.checking || unavailableReason !== null}
            onClick={compact}
            type="button"
          >
            Compact
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}

/**
 * Gauge button for a known reading: tooltip with usage and age, click opens
 * the compaction confirmation.
 */
export function ContextGaugeButton({
  agentName,
  className,
  dialSize = 14,
  reading,
}: {
  agentName: string;
  className?: string;
  dialSize?: number;
  reading: AgentContextUsage;
}) {
  const [open, setOpen] = React.useState(false);
  const usageLabel = formatContextUsageLabel(reading.used, reading.size);
  const updatedAgo = formatContextUpdatedAgo(reading.updatedAt);
  return (
    <>
      <Tooltip>
        <TooltipTrigger asChild>
          <button
            aria-label={`${agentName} context: ${usageLabel}, updated ${updatedAgo}. Compact session`}
            className={cn(
              "flex items-center justify-center rounded-full focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring",
              className,
            )}
            data-testid="agent-context-gauge"
            onClick={() => setOpen(true)}
            type="button"
          >
            <ContextGaugeDial size={dialSize} reading={reading} />
          </button>
        </TooltipTrigger>
        <TooltipContent>
          <p>{usageLabel}</p>
          <p className="text-xs opacity-80">Updated {updatedAgo}</p>
        </TooltipContent>
      </Tooltip>
      {open ? (
        <CompactSessionDialog
          agentName={agentName}
          onOpenChange={setOpen}
          open={open}
          reading={reading}
        />
      ) : null}
    </>
  );
}

/**
 * Chat-avatar gauge for an agent message. Renders nothing until the store
 * holds a reading for the message's thread scope or the channel's whole
 * conversation scope.
 */
export function MessageAgentContextGauge({
  agentName,
  agentPubkey,
  channelId,
  className,
  threadRootCandidate,
}: {
  agentName: string;
  agentPubkey: string;
  channelId: string;
  className?: string;
  threadRootCandidate: string | null;
}) {
  const enabled = useContextGaugeEnabled();
  const reading = useResolvedAgentContextUsage(
    agentPubkey,
    channelId,
    threadRootCandidate,
  );
  if (!enabled || !reading) return null;
  return (
    <ContextGaugeButton
      agentName={agentName}
      className={className}
      reading={reading}
    />
  );
}
