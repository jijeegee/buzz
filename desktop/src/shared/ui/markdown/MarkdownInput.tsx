import { useT } from "@/shared/i18n";
import type * as React from "react";

import { cn } from "@/shared/lib/cn";
import { Checkbox } from "@/shared/ui/checkbox";

type MarkdownInputProps = React.ComponentProps<"input"> & {
  node?: unknown;
};

export function MarkdownInput({
  checked,
  className,
  node: _node,
  type,
  ...props
}: MarkdownInputProps) {
  const t = useT();
  if (type === "checkbox") {
    return (
      <Checkbox
        aria-label={
          checked ? t("ui.markdown.taskDone") : t("ui.markdown.taskNotDone")
        }
        checked={Boolean(checked)}
        className={cn(
          "pointer-events-none mr-1.5 inline-flex align-[-0.125rem] disabled:opacity-45",
          className,
        )}
        disabled
        tabIndex={-1}
      />
    );
  }

  return <input {...props} className={className} type={type} />;
}
