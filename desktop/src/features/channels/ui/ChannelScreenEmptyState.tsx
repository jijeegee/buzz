import { useT } from "@/shared/i18n";
export function ChannelScreenEmptyState() {
  const t = useT();
  return (
    <div className="flex min-h-0 flex-1 items-center justify-center px-6 py-8">
      <p className="text-sm text-muted-foreground">
        {t("channels.emptyState")}
      </p>
    </div>
  );
}
