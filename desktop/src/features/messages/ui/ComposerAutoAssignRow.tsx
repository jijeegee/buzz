import { X } from "lucide-react";

import type { AutoAssignSuggestion } from "./useAutoAssign";

/**
 * Smart routing in the composer: the "→ Name" chip(s) for the agent the
 * router picked before Enter, the pending dots while a slow call runs, and
 * the quiet notice after a send that went out unassigned. Renders nothing
 * when there is nothing to say.
 *
 * One label owner per control: each remove button carries the only
 * actionable label; the visible name and arrow are not interactive.
 */
export function ComposerAutoAssignRow({
  announcement,
  notice,
  onRemove,
  pending,
  suggestions,
}: {
  announcement: string;
  notice: string | null;
  onRemove: (pubkey: string) => void;
  pending: boolean;
  suggestions: readonly AutoAssignSuggestion[];
}) {
  const hasChips = suggestions.length > 0;
  return (
    <>
      <output
        aria-live="polite"
        className="sr-only"
        data-testid="composer-auto-assign-status"
      >
        {hasChips ? announcement : ""}
      </output>
      {hasChips || pending || notice ? (
        <div
          className="mb-2 flex flex-wrap items-center gap-1.5 text-xs"
          data-testid="composer-auto-assign"
        >
          {suggestions.map((suggestion) => (
            <span
              className="inline-flex items-center gap-1 rounded-full bg-primary/10 py-0.5 pl-2 pr-0.5 font-medium text-primary"
              data-testid={`composer-auto-assign-${suggestion.pubkey}`}
              key={suggestion.pubkey}
            >
              <span aria-hidden="true">→</span>
              <span>{suggestion.name}</span>
              <button
                aria-label={`Auto-assigned to ${suggestion.name} — remove`}
                className="inline-flex h-4 w-4 items-center justify-center rounded-full hover:bg-primary/20 focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring"
                data-testid={`composer-auto-assign-remove-${suggestion.pubkey}`}
                onClick={() => onRemove(suggestion.pubkey)}
                onMouseDown={(event) => event.preventDefault()}
                type="button"
              >
                <X aria-hidden="true" className="h-3 w-3" />
              </button>
            </span>
          ))}
          {pending && !hasChips ? (
            <span
              aria-busy="true"
              aria-label="Choosing an agent"
              className="inline-flex gap-0.5 px-1 text-muted-foreground"
              data-testid="composer-auto-assign-pending"
              role="status"
            >
              <span aria-hidden="true" className="animate-pulse">
                •••
              </span>
            </span>
          ) : null}
          {notice ? (
            <span
              className="text-muted-foreground"
              data-testid="composer-auto-assign-notice"
            >
              {notice}
            </span>
          ) : null}
        </div>
      ) : null}
    </>
  );
}
