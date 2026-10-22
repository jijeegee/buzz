import { cn } from "@/shared/lib/cn";
import { useT } from "@/shared/i18n";

type PersonaAddedByProps = {
  className?: string;
  label?: string;
};

export function PersonaAddedBy({ className, label }: PersonaAddedByProps) {
  const t = useT();
  return (
    <p className={cn("truncate text-xs leading-tight", className)}>
      {t("agents.addedBy", {
        name: label ?? t("common.you"),
        muted: (chunks) => (
          <span className="text-muted-foreground/55">{chunks}</span>
        ),
        b: (chunks) => <span className="text-muted-foreground">{chunks}</span>,
      })}
    </p>
  );
}
