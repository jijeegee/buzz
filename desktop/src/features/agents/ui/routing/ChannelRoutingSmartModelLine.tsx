import { ChevronRight } from "lucide-react";

import type { TaskModelStatus } from "@/shared/api/tauriMessageRouting";

/**
 * The grey line under the Smart routing radio: which model routes, or why
 * it can't, with a link to Settings › Models. The model UI itself lives
 * only there (Task models row); this line never duplicates it.
 */
export function ChannelRoutingSmartModelLine({
  onOpenModels,
  status,
  statusError,
}: {
  onOpenModels: (tab: "providers" | "tasks") => void;
  status: TaskModelStatus | undefined;
  statusError: boolean;
}) {
  if (!status) {
    return statusError ? (
      <p
        className="ml-7 text-sm text-destructive"
        data-testid="agents-channel-routing-smart-model"
      >
        Couldn't check the routing model.
      </p>
    ) : null;
  }
  const ready = status.ready;
  return (
    <p
      className="ml-7 flex flex-wrap items-center gap-x-1 text-sm text-muted-foreground sm:pl-[7.75rem]"
      data-testid="agents-channel-routing-smart-model"
    >
      <span>
        {ready
          ? `Uses ${status.modelLabel ?? status.effectiveModel}`
          : (status.notReadyReason ??
            "Sign in to Codex or Claude Code, or add an API key")}
      </span>
      <span aria-hidden="true">·</span>
      <button
        className="inline-flex items-center gap-0.5 font-medium text-primary hover:underline focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring"
        data-testid="agents-channel-routing-smart-model-link"
        onClick={() => onOpenModels(ready ? "tasks" : "providers")}
        type="button"
      >
        {ready ? "Change in Models" : "Models"}
        <ChevronRight aria-hidden="true" className="h-3.5 w-3.5" />
      </button>
    </p>
  );
}
