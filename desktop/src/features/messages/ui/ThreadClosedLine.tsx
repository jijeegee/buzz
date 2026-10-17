import { CircleCheck } from "lucide-react";

/** "Mina closed this thread · Reopen", under the last reply of a closed thread. */
export function ThreadClosedLine({
  closerName,
  onReopen,
  reopenPending,
}: {
  closerName: string | null;
  onReopen?: () => void;
  reopenPending?: boolean;
}) {
  return (
    <section
      aria-label="Thread closed"
      className="mt-3 flex items-center gap-2 rounded-md border border-border/60 bg-muted/40 px-3 py-2 text-sm text-muted-foreground"
      data-testid="task-thread-closed-banner"
    >
      <CircleCheck aria-hidden className="size-4 shrink-0 text-primary" />
      <span className="min-w-0">
        {closerName ? `${closerName} closed this thread` : "Thread closed"}
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
              Reopen
            </button>
          </>
        ) : null}
      </span>
    </section>
  );
}
