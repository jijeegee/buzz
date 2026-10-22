import { ChevronDown } from "lucide-react";
import * as React from "react";

import type { ProjectDetailAgentContext } from "@/features/projects/lib/projectDetailAgentContext";
import { cn } from "@/shared/lib/cn";
import { Button } from "@/shared/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/shared/ui/dropdown-menu";
import { type MessageKey, useT } from "@/shared/i18n";

type SelectionItem = NonNullable<
  ProjectDetailAgentContext["selection"]
>[number];

const MAX_VISIBLE_ITEMS = 4;

const KIND_KEYS: Partial<Record<string, MessageKey>> = {
  channel: "projects.selectionKind.channel",
  commit: "projects.selectionKind.commit",
  project: "projects.selectionKind.project",
  repository: "projects.selectionKind.repository",
  review: "projects.selectionKind.review",
  task: "projects.selectionKind.task",
};

function SelectionItemRow({
  item,
  overflow = false,
}: {
  item: SelectionItem;
  overflow?: boolean;
}) {
  const t = useT();
  const kindKey = KIND_KEYS[item.kind];
  return (
    <div
      className="flex min-w-0 items-baseline gap-2 text-xs"
      data-testid={
        overflow
          ? "projects-agent-selection-overflow-item"
          : "projects-agent-selection-item"
      }
    >
      <span className="shrink-0 capitalize text-muted-foreground">
        {kindKey ? t(kindKey) : item.kind}
      </span>
      <span className="min-w-0 truncate text-foreground" title={item.title}>
        {item.title}
      </span>
    </div>
  );
}

export function ProjectAgentSelectionComposerBanner({
  items,
}: {
  items: NonNullable<ProjectDetailAgentContext["selection"]>;
}) {
  const t = useT();
  const [expanded, setExpanded] = React.useState(true);
  const visibleItems = items.slice(0, MAX_VISIBLE_ITEMS);
  const overflowItems = items.slice(MAX_VISIBLE_ITEMS);
  const summary = t("projects.agentSelection.summary", {
    count: items.length,
  });

  return (
    <div className="px-3" data-testid="projects-agent-selection-context">
      <section className="relative z-0 -mb-4 rounded-t-2xl border border-b-0 border-border/60 bg-muted/55 px-3 pb-6 pt-2.5 text-muted-foreground backdrop-blur-sm">
        <button
          aria-expanded={expanded}
          className="flex w-full min-w-0 items-center gap-2 text-left text-xs font-medium text-foreground"
          data-testid="projects-agent-selection-toggle"
          onClick={() => setExpanded((current) => !current)}
          type="button"
        >
          <ChevronDown
            className={cn(
              "h-3.5 w-3.5 shrink-0 text-muted-foreground transition-transform",
              !expanded && "-rotate-90",
            )}
          />
          <span className="truncate">{summary}</span>
        </button>
        {expanded ? (
          <div className="mt-2 space-y-1.5 pl-5.5">
            {visibleItems.map((item) => (
              <SelectionItemRow item={item} key={item.id} />
            ))}
            {overflowItems.length > 0 ? (
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <Button
                    className="h-6 px-2 text-xs text-muted-foreground"
                    data-testid="projects-agent-selection-more"
                    size="sm"
                    type="button"
                    variant="ghost"
                  >
                    {t("projects.agentSelection.more", {
                      count: overflowItems.length,
                    })}
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent
                  align="start"
                  className="max-h-64 min-w-64 overflow-y-auto"
                  side="top"
                >
                  {overflowItems.map((item) => (
                    <DropdownMenuItem disabled key={item.id}>
                      <SelectionItemRow item={item} overflow />
                    </DropdownMenuItem>
                  ))}
                </DropdownMenuContent>
              </DropdownMenu>
            ) : null}
          </div>
        ) : null}
      </section>
    </div>
  );
}
