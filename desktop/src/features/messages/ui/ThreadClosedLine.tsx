import { useT } from "@/shared/i18n";
import { CircleCheck } from "lucide-react";

/** "Mina closed this thread · Reopen", under the last reply of a closed thread. */
export function ThreadClosedLine({
  closerName,
  onReopen,
  reopenPending,
  taskBanner = false,
}: {
  closerName: string | null;
  onReopen?: () => void;
  reopenPending?: boolean;
  /** The Projects task banner, unchanged from before the Close toggle. */
  taskBanner?: boolean;
}) {
  const t = useT();
  if (taskBanner) {
    return (
      <section
        aria-label={t("messages.threadClosed.taskClosed")}
        className="mt-3 flex items-center gap-2 rounded-md border border-border/60 bg-muted/40 px-3 py-2 text-sm text-muted-foreground"
        data-testid="task-thread-closed-banner"
      >
        <CircleCheck aria-hidden className="size-4 shrink-0 text-primary" />
        <span>{t("messages.threadClosed.taskBanner")}</span>
      </section>
    );
  }
  return (
    <section
      aria-label={t("messages.threadClosed.closed")}
      className="mt-3 flex items-center gap-2 rounded-md border border-border/60 bg-muted/40 px-3 py-2 text-sm text-muted-foreground"
      data-testid="task-thread-closed-banner"
    >
      <CircleCheck aria-hidden className="size-4 shrink-0 text-primary" />
      <span className="min-w-0">
        {closerName
          ? t("messages.threadClosed.closedBy", { name: closerName })
          : t("messages.threadClosed.closed")}
        {onReopen ? (
          <>
            {" · "}
            <button
              className="font-medium text-foreground hover:underline disabled:opacity-50"
              data-testid="thread-closed-reopen"
              disabled={reopenPending}
              onClick={onReopen}
              type="button"
            >
              {t("messages.threadClosed.reopen")}
            </button>
          </>
        ) : null}
      </span>
    </section>
  );
}
