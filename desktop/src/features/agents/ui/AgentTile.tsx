import type { ReactNode } from "react";
import { EllipsisVertical } from "lucide-react";

import { ProfileAvatar } from "@/features/profile/ui/ProfileAvatar";
import { cn } from "@/shared/lib/cn";
import { DropdownMenuTrigger } from "@/shared/ui/dropdown-menu";
import { useT, type MessageKey } from "@/shared/i18n";

export type AgentTileStatus = "working" | "running" | "stopped" | "error";

const STATUS_LABEL: Record<AgentTileStatus, MessageKey> = {
  working: "agents.tile.working",
  running: "agents.tile.running",
  stopped: "agents.tile.stopped",
  error: "agents.tile.error",
};

const STATUS_CLASS: Record<AgentTileStatus, string> = {
  working: "bg-amber-500/15 text-amber-700 dark:text-amber-400",
  running: "bg-emerald-500/15 text-emerald-700 dark:text-emerald-400",
  stopped: "bg-muted text-muted-foreground",
  error: "bg-destructive/15 text-destructive",
};

export const AGENT_TILE_GRID_CLASS =
  "grid w-full grid-cols-1 gap-3 [@container(min-width:34rem)]:grid-cols-2 [@container(min-width:52rem)]:grid-cols-3";

/**
 * One card on the Agents page: picture, name (after the device robot for
 * real agents), a second line, a status or kind pill, a description line and
 * a row of buttons. Clicking anywhere outside the buttons opens it.
 */
export function AgentTile({
  actions,
  avatarUrl,
  badge,
  buttons,
  dataTestId,
  description,
  label,
  labelBadge,
  openLabel,
  status,
  subtitle,
  onOpen,
}: {
  /** The ⋯ menu, top right. */
  actions?: ReactNode;
  avatarUrl: string | null | undefined;
  /** A kind pill ("Preset") where a real agent shows its status. */
  badge?: string;
  buttons: ReactNode;
  dataTestId: string;
  description?: string | null;
  label: string;
  labelBadge?: ReactNode;
  openLabel: string;
  status?: AgentTileStatus;
  subtitle?: string | null;
  onOpen: () => void;
}) {
  const t = useT();
  return (
    <div
      className="relative flex min-w-0 flex-col gap-3 rounded-xl border border-border/70 bg-background p-3 shadow-xs transition-colors hover:border-border"
      data-testid={dataTestId}
    >
      <button
        aria-label={openLabel}
        className="absolute inset-0 z-0 rounded-xl focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring"
        onClick={onOpen}
        type="button"
      />
      <div className="pointer-events-none relative flex min-w-0 items-start gap-2.5">
        <ProfileAvatar
          mediaVariant="small"
          avatarUrl={avatarUrl?.trim() || null}
          className="h-9 w-9 shrink-0 text-xs shadow-none"
          iconClassName="h-4 w-4"
          label={label}
          shape="circle"
        />
        <div className="flex min-w-0 flex-1 flex-col">
          <span className="flex min-w-0 items-center gap-1">
            {labelBadge ? (
              <span className="pointer-events-auto flex shrink-0 empty:hidden">
                {labelBadge}
              </span>
            ) : null}
            <span className="min-w-0 truncate text-sm font-semibold text-foreground">
              {label}
            </span>
          </span>
          {subtitle ? (
            <span className="truncate text-xs text-muted-foreground">
              {subtitle}
            </span>
          ) : null}
        </div>
        {status ? (
          <span
            className={cn(
              "shrink-0 rounded-full px-2 py-0.5 text-2xs font-medium",
              STATUS_CLASS[status],
            )}
            data-testid={`${dataTestId}-status`}
          >
            {t(STATUS_LABEL[status])}
          </span>
        ) : badge ? (
          <span className="shrink-0 rounded-full bg-muted px-2 py-0.5 text-2xs font-medium text-muted-foreground">
            {badge}
          </span>
        ) : null}
        {actions ? (
          <span className="pointer-events-auto -mr-1 -mt-1 shrink-0">
            {actions}
          </span>
        ) : null}
      </div>
      {description ? (
        <p className="pointer-events-none relative line-clamp-2 min-h-8 text-xs text-muted-foreground">
          {description}
        </p>
      ) : (
        <div className="min-h-8" />
      )}
      <div className="relative mt-auto flex flex-wrap gap-1.5">{buttons}</div>
    </div>
  );
}

/** Small device heading on the Agents page: robot, name, muted detail. */
export function DeviceHeading({
  detail,
  robot,
  testId,
  title,
}: {
  detail?: string;
  robot?: ReactNode;
  testId?: string;
  title: string;
}) {
  return (
    <h3
      className="flex min-w-0 items-center gap-1.5 text-sm font-semibold text-foreground"
      data-testid={testId}
    >
      {robot ? <span className="flex h-5 w-5 shrink-0">{robot}</span> : null}
      <span className="truncate">{title}</span>
      {detail ? (
        <span className="truncate text-xs font-normal text-muted-foreground">
          {detail}
        </span>
      ) : null}
    </h3>
  );
}

/** The ⋯ button that opens a tile's menu; put inside a `DropdownMenu`. */
export function TileMenuTrigger({ label }: { label: string }) {
  return (
    <DropdownMenuTrigger asChild>
      <button
        aria-label={label}
        className="flex h-7 w-7 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
        type="button"
      >
        <EllipsisVertical className="h-4 w-4" />
      </button>
    </DropdownMenuTrigger>
  );
}
