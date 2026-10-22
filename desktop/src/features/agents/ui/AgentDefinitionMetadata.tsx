import { cn } from "@/shared/lib/cn";
import { useT } from "@/shared/i18n";

export function AgentDefinitionMetadata({
  className,
  isBuiltIn,
  model,
  provider,
  runtime,
}: {
  className?: string;
  isBuiltIn: boolean;
  model: string | null;
  provider?: string | null;
  runtime: string | null;
}) {
  const t = useT();
  const items = [
    {
      label: t("agents.metadata.type"),
      value: isBuiltIn
        ? t("agents.metadata.builtIn")
        : t("agents.metadata.custom"),
    },
    {
      label: t("agents.metadata.model"),
      value: model ?? t("agents.metadata.appDefault"),
    },
    {
      label: t("agents.metadata.runtime"),
      value: runtime ?? t("agents.metadata.appDefault"),
    },
    ...(provider !== undefined
      ? [
          {
            label: t("agents.metadata.provider"),
            value: provider ?? t("agents.metadata.appDefault"),
          },
        ]
      : []),
  ];

  return (
    <div
      className={cn("rounded-lg border border-border/70 bg-card/70", className)}
      data-testid="agent-definition-metadata"
    >
      <div
        className={cn(
          "grid",
          items.length > 3 ? "sm:grid-cols-4" : "sm:grid-cols-3",
        )}
      >
        {items.map((item, index) => (
          <div
            className={cn(
              "relative px-4 py-3",
              index > 0 &&
                "border-t border-border/60 sm:border-t-0 sm:before:absolute sm:before:bottom-3 sm:before:left-0 sm:before:top-3 sm:before:w-px sm:before:bg-border/70",
            )}
            key={item.label}
          >
            <p className="text-xs font-semibold text-muted-foreground">
              {item.label}
            </p>
            <p className="mt-2 text-sm font-medium text-foreground">
              {item.value}
            </p>
          </div>
        ))}
      </div>
    </div>
  );
}
