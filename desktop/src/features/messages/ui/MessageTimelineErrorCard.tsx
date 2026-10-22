import { useT } from "@/shared/i18n";
import { Button } from "@/shared/ui/button";

/** Retryable terminal failure for a channel history load with no cached rows. */
export function MessageTimelineErrorCard({
  onRetry,
}: {
  onRetry?: () => void;
}) {
  const t = useT();
  return (
    <div
      className="mt-auto rounded-2xl border border-dashed border-destructive/50 bg-destructive/5 px-6 py-10 text-center shadow-xs"
      data-testid="message-timeline-error"
      role="alert"
    >
      <p className="text-base font-semibold tracking-tight">
        {t("messages.timeline.loadFailedTitle")}
      </p>
      <p className="mt-2 text-sm text-muted-foreground">
        {t("messages.timeline.loadFailedBody")}
      </p>
      {onRetry ? (
        <Button
          className="mt-4"
          data-testid="message-timeline-retry"
          onClick={onRetry}
          size="sm"
          type="button"
          variant="outline"
        >
          {t("common.retry")}
        </Button>
      ) : null}
    </div>
  );
}
