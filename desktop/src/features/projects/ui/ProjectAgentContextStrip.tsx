import { X } from "lucide-react";

import type { ProjectDetailAgentContext } from "@/features/projects/lib/projectDetailAgentContext";
import { Button } from "@/shared/ui/button";
import { PROJECT_COLUMN_HEADER_BACKDROP_CLASS } from "./projectPanelStyles";
import { type Translate, useT } from "@/shared/i18n";

function contextLabel(context: ProjectDetailAgentContext, t: Translate) {
  if (context.selection?.length) {
    return t("projects.agentContextStrip.agentChat");
  }
  if (context.workItem) {
    return context.workItem.title;
  }
  if (context.file?.path) {
    return context.file.path.split("/").filter(Boolean).at(-1) ?? context.view;
  }
  return context.view;
}

export function ProjectAgentContextStrip({
  context,
  onClose,
  sharedBackdrop = false,
}: {
  context: ProjectDetailAgentContext;
  onClose?: () => void;
  sharedBackdrop?: boolean;
}) {
  const t = useT();
  const label = contextLabel(context, t);

  return (
    <div
      className={`absolute inset-x-0 top-0 z-30 flex h-13 shrink-0 items-center px-4 ${
        sharedBackdrop ? "" : PROJECT_COLUMN_HEADER_BACKDROP_CLASS
      }`}
      data-testid="project-agent-context"
      title={label}
    >
      <p className="min-w-0 flex-1 truncate text-sm font-medium text-foreground">
        {label}
      </p>
      {onClose ? (
        <Button
          aria-label={t("projects.agentContextStrip.close")}
          className="h-7 w-7 shrink-0"
          onClick={onClose}
          size="icon"
          title={t("projects.agentContextStrip.close")}
          type="button"
          variant="ghost"
        >
          <X className="h-3.5 w-3.5" />
        </Button>
      ) : null}
    </div>
  );
}
