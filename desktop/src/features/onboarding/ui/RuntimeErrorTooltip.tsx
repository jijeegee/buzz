import { AlertTriangle } from "lucide-react";

import { useT } from "@/shared/i18n";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/shared/ui/tooltip";

type RuntimeErrorTooltipProps = {
  className?: string;
  detail: string;
  label: string;
  showIcon?: boolean;
  testId?: string;
};

export function RuntimeErrorTooltip({
  className,
  detail,
  label,
  showIcon = false,
  testId,
}: RuntimeErrorTooltipProps) {
  const t = useT();
  return (
    <Tooltip disableHoverableContent={false}>
      <TooltipTrigger asChild>
        <span
          aria-label={t("onboarding.runtimeError.ariaLabel", { label, detail })}
          className={className}
          data-testid={testId}
          role="status"
          // biome-ignore lint/a11y/noNoninteractiveTabindex: Focus exposes the error tooltip to keyboard users without adding a nested card action
          tabIndex={0}
        >
          {showIcon ? (
            <AlertTriangle
              aria-hidden="true"
              className="h-3.5 w-3.5 shrink-0"
            />
          ) : null}
          <span className="min-w-0 truncate">{label}</span>
        </span>
      </TooltipTrigger>
      <TooltipContent
        className="pointer-events-auto max-w-80 text-left"
        side="bottom"
        sideOffset={12}
      >
        <span className="block max-h-48 overflow-y-auto overflow-x-hidden break-words whitespace-pre-line leading-4">
          {detail}
        </span>
      </TooltipContent>
    </Tooltip>
  );
}
