import { RefreshCw } from "lucide-react";

import { Badge } from "@/shared/ui/badge";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/shared/ui/tooltip";
import type { RestartDiffEntry, RestartChange } from "@/shared/api/types";
import { cn } from "@/shared/lib/cn";
import { useT } from "@/shared/i18n";

// ── Label helpers ─────────────────────────────────────────────────────────────

/**
 * Humanise a serde dotted-path field id into a display label.
 * `snake_case` → `"Snake case"`, `env.FOO` → `"FOO (env)"`.
 * Unknown paths render as-is — no per-field label map.
 */
function humaniseFieldId(field: string): string {
  if (field.startsWith("env.")) {
    const key = field.slice(4);
    return `${key} (env)`;
  }
  // snake_case → words, sentence case
  return field.replace(/_/g, " ").replace(/\b\w/, (c) => c.toUpperCase());
}

// ── Change description ────────────────────────────────────────────────────────

function formatJsonValue(v: unknown): string {
  if (v === null) return "null";
  if (v === undefined) return "—";
  if (typeof v === "string") return v;
  return JSON.stringify(v);
}

function ChangeDescription({ change }: { change: RestartChange }) {
  const t = useT();
  switch (change.kind) {
    case "value":
      return (
        <span>
          <span className="line-through opacity-60">
            {formatJsonValue(change.before)}
          </span>
          {" → "}
          <span>{formatJsonValue(change.after)}</span>
        </span>
      );
    case "masked":
      return (
        <span>
          <span className="line-through opacity-60">
            {change.before ?? "••••"}
          </span>
          {" → "}
          <span>{change.after ?? "••••"}</span>
        </span>
      );
    case "added":
      return <span>{t("agents.restartDiff.added")}</span>;
    case "removed":
      return <span>{t("agents.restartDiff.removed")}</span>;
    default:
      // Unknown kind — render nothing, remain type-safe at runtime
      return null;
  }
}

// ── Diff list ─────────────────────────────────────────────────────────────────

const TOOLTIP_CAP = 6;

/**
 * `tooltip` — renders inside the semantic secondary tooltip surface; uses
 *   `text-secondary-foreground` variants for contrast there.
 * `inline`  — renders inside the amber Runtime banner or other light
 *   surfaces; inherits foreground from the container instead.
 */
function DiffList({
  entries,
  cap,
  variant = "tooltip",
}: {
  entries: RestartDiffEntry[];
  cap?: number;
  variant?: "tooltip" | "inline";
}) {
  const t = useT();
  const visible = cap !== undefined ? entries.slice(0, cap) : entries;
  const overflow =
    cap !== undefined && entries.length > cap ? entries.length - cap : 0;

  const valueClass =
    variant === "tooltip" ? "text-secondary-foreground/80" : "text-foreground";
  const overflowClass =
    variant === "tooltip"
      ? "text-secondary-foreground/60"
      : "text-muted-foreground";

  return (
    <ul className="space-y-1">
      {visible.map((entry) => (
        <li className="flex items-baseline gap-1.5" key={entry.field}>
          <span className="shrink-0 font-medium">
            {humaniseFieldId(entry.field)}:
          </span>
          <span className={valueClass}>
            <ChangeDescription change={entry.change} />
          </span>
        </li>
      ))}
      {overflow > 0 ? (
        <li className={overflowClass}>
          {t("agents.restartDiff.more", { count: overflow })}
        </li>
      ) : null}
    </ul>
  );
}

// ── Badge + tooltip ───────────────────────────────────────────────────────────

/**
 * The restart-required badge. When `restartDiff` is non-empty, shows a hover
 * tooltip with the itemised before→after diff (capped at {@link TOOLTIP_CAP}
 * entries) plus the auto-restart blurb. Renders as a non-interactive `<span>`
 * so it can safely be placed adjacent to (not inside) a `<button>` — it must
 * never be a descendant of an interactive element.
 */
export function RestartDiffBadge({
  autoRestartEnabled = false,
  restartDiff,
  className,
}: {
  autoRestartEnabled?: boolean;
  restartDiff: RestartDiffEntry[];
  className?: string;
}) {
  const t = useT();
  const badge = (
    <Badge className={cn("cursor-default gap-1", className)} variant="warning">
      <RefreshCw className="h-3 w-3" />
      {t("agents.restartDiff.badge")}
    </Badge>
  );

  if (restartDiff.length === 0) {
    // No diff data — render plain badge without tooltip.
    return badge;
  }

  return (
    <Tooltip>
      {/* asChild renders the trigger as the Badge's <span>, which is a valid
          non-interactive element. No nested button. */}
      <TooltipTrigger asChild>
        <Badge
          className={cn("cursor-default gap-1", className)}
          variant="warning"
          tabIndex={0}
          data-testid="restart-diff-badge"
        >
          <RefreshCw className="h-3 w-3" />
          {t("agents.restartDiff.badge")}
        </Badge>
      </TooltipTrigger>
      <TooltipContent className="max-w-72 text-xs" side="bottom">
        <p className="mb-1.5 font-semibold">{t("agents.restartDiff.title")}</p>
        <DiffList cap={TOOLTIP_CAP} entries={restartDiff} />
        <p className="mt-1.5 text-secondary-foreground/70">
          {autoRestartEnabled
            ? t("agents.restartDiff.autoOn")
            : t("agents.restartDiff.autoOff")}
        </p>
      </TooltipContent>
    </Tooltip>
  );
}

/**
 * Full uncapped diff list for the Runtime-tab banner. Renders inline; no
 * tooltip wrapper.
 */
export function RestartDiffList({
  restartDiff,
}: {
  restartDiff: RestartDiffEntry[];
}) {
  if (restartDiff.length === 0) return null;
  return (
    <div
      className="mt-2 text-xs text-muted-foreground"
      data-testid="restart-diff-list"
    >
      <DiffList entries={restartDiff} variant="inline" />
    </div>
  );
}
