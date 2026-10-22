import { AlertCircle, Lock, Upload } from "lucide-react";

import type {
  AgentSnapshotImportPreview,
  AgentSnapshotImportResult,
} from "@/features/agents/hooks";
import { Button } from "@/shared/ui/button";
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/shared/ui/dialog";
import { Separator } from "@/shared/ui/separator";

import { AgentDefinitionMetadata } from "./AgentDefinitionMetadata";
import {
  getIntl,
  type MessageKey,
  type MessageValues,
  type Translate,
  useT,
} from "@/shared/i18n";

// ── Types ─────────────────────────────────────────────────────────────────────

type ImportPhase = "preview" | "confirming" | "result";

type AgentSnapshotImportDialogProps = {
  open: boolean;
  /** Preview data loaded by the caller before opening. */
  preview: AgentSnapshotImportPreview;
  /** True while the confirm mutation is in-flight. */
  isConfirming: boolean;
  /** Set when the confirm mutation has returned a result. */
  result: AgentSnapshotImportResult | null;
  /** Error from the confirm mutation, if any. */
  confirmError: string | null;
  /** Called when user clicks Import. */
  onConfirm: () => void;
  onOpenChange: (open: boolean) => void;
};

// ── Component ─────────────────────────────────────────────────────────────────

export function AgentSnapshotImportDialog({
  open,
  preview,
  isConfirming,
  result,
  confirmError,
  onConfirm,
  onOpenChange,
}: AgentSnapshotImportDialogProps) {
  const t = useT();
  const phase: ImportPhase =
    result !== null ? "result" : isConfirming ? "confirming" : "preview";

  const hasMemory = preview.memoryEntryCount > 0;
  const memoryLevelLabel =
    preview.memoryLevel === "core"
      ? "core"
      : preview.memoryLevel === "everything"
        ? "all"
        : "none";

  return (
    <Dialog onOpenChange={onOpenChange} open={open}>
      <DialogContent
        aria-describedby={undefined}
        className="max-h-[85vh] max-w-2xl overflow-y-auto"
        data-testid="agent-snapshot-import-dialog"
        showCloseButton={false}
      >
        <DialogHeader className="space-y-0">
          <div className="flex items-center justify-between gap-4">
            <DialogTitle>
              {phase === "result"
                ? t("agents.import.doneTitle")
                : t("agents.import.title")}
            </DialogTitle>
            <div className="flex items-center gap-2">
              {phase === "preview" ? (
                <>
                  <Button
                    data-testid="agent-snapshot-import-confirm"
                    disabled={isConfirming}
                    onClick={() => onConfirm()}
                    size="sm"
                    type="button"
                    variant="default"
                  >
                    <Upload className="h-4 w-4" />
                    {t("agents.import.import")}
                  </Button>
                  <DialogClose asChild>
                    <Button
                      disabled={isConfirming}
                      size="sm"
                      type="button"
                      variant="ghost"
                    >
                      {t("common.cancel")}
                    </Button>
                  </DialogClose>
                </>
              ) : (
                <DialogClose asChild>
                  <Button size="sm" type="button" variant="ghost">
                    {t("common.close")}
                  </Button>
                </DialogClose>
              )}
            </div>
          </div>
        </DialogHeader>

        <Separator />

        {phase === "preview" ? (
          <PreviewBody
            preview={preview}
            hasMemory={hasMemory}
            memoryLevelLabel={memoryLevelLabel}
          />
        ) : phase === "confirming" ? (
          <div className="py-4 text-center text-sm text-muted-foreground">
            {t("agents.import.creating")}
          </div>
        ) : result !== null ? (
          <ResultBody result={result} confirmError={confirmError} />
        ) : null}
      </DialogContent>
    </Dialog>
  );
}

// ── Preview body ──────────────────────────────────────────────────────────────

/**
 * PreviewBody and ResultBody are also called as plain functions (outside a
 * React render), so they translate through the app-wide intl, not `useT()`.
 * They always render inside the dialog, which re-renders on language change.
 */
const translate = ((key: MessageKey, values?: MessageValues) =>
  getIntl().formatMessage({ id: key }, values)) as unknown as Translate;

export function PreviewBody({
  preview,
  hasMemory,
  memoryLevelLabel,
}: {
  preview: AgentSnapshotImportPreview;
  hasMemory: boolean;
  memoryLevelLabel: string;
}) {
  const t = translate;
  return (
    <div className="space-y-4 py-1">
      {/* Agent identity */}
      <div className="space-y-1">
        <p className="text-sm font-medium">{preview.displayName}</p>
      </div>

      {/* Locked-card provenance: this file was encrypted to this machine's
          keys and has been unlocked for review. */}
      {preview.locked ? (
        <div
          className="flex items-start gap-2 rounded-md border border-border bg-muted/40 px-3 py-2 text-sm"
          data-testid="agent-snapshot-import-locked-notice"
        >
          <Lock className="mt-0.5 h-4 w-4 shrink-0" />
          <p>
            {t("agents.import.lockedNotice", {
              b: (chunks) => <strong>{chunks}</strong>,
            })}
          </p>
        </div>
      ) : null}

      <AgentDefinitionMetadata
        isBuiltIn={preview.isBuiltIn}
        model={preview.model}
        runtime={preview.runtime}
      />

      {/* Portable behavior — never hide executable configuration behind a summary. */}
      <section
        className="space-y-2 rounded-md border border-border p-3"
        data-testid="agent-snapshot-import-behavior"
      >
        <div>
          <p className="text-sm font-medium">
            {t("agents.dialog.instructions")}
          </p>
          <p className="text-xs text-muted-foreground">
            {t("agents.import.reviewInstructions")}
          </p>
        </div>
        <pre
          className="max-h-48 overflow-auto whitespace-pre-wrap break-words rounded bg-muted/60 p-3 text-xs"
          data-testid="agent-snapshot-import-system-prompt"
        >
          {preview.systemPrompt || t("agents.import.noSystemPrompt")}
        </pre>
      </section>

      <p className="text-sm text-muted-foreground">
        {t("agents.import.freshIdentity")}
      </p>

      {/* Memory section */}
      {hasMemory ? (
        <div
          className="flex items-start gap-2 rounded-md border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-sm text-amber-700 dark:text-amber-400"
          data-testid="agent-snapshot-import-memory-warning"
        >
          <AlertCircle className="mt-0.5 h-4 w-4 shrink-0" />
          <p>
            {t("agents.import.memoryWarning", {
              count: preview.memoryEntryCount,
              level: memoryLevelLabel,
              b: (chunks) => <strong>{chunks}</strong>,
            })}
          </p>
        </div>
      ) : (
        <p className="text-xs text-muted-foreground">
          {t("agents.import.noMemory")}
        </p>
      )}

      <details
        className="rounded-md border border-border p-3"
        data-testid="agent-snapshot-import-manifest"
      >
        <summary className="cursor-pointer text-sm font-medium">
          {t("agents.import.manifest")}
        </summary>
        <p className="mt-2 text-xs text-muted-foreground">
          {t("agents.import.manifestHelp")}
        </p>
        <pre className="mt-2 max-h-64 overflow-auto whitespace-pre-wrap break-words rounded bg-muted/60 p-3 text-xs">
          {preview.manifestJson}
        </pre>
      </details>
    </div>
  );
}

// ── Result body ───────────────────────────────────────────────────────────────

export function ResultBody({
  result,
  confirmError,
}: {
  result: AgentSnapshotImportResult;
  confirmError: string | null;
}) {
  const t = translate;
  const hasPartialMemory =
    result.memoryTotal > 0 && result.memoryWritten < result.memoryTotal;

  return (
    <div className="space-y-3 py-1">
      <p className="text-sm">
        {t("agents.import.created", {
          name: result.displayName,
          b: (chunks) => <span className="font-medium">{chunks}</span>,
        })}
      </p>

      {result.memoryTotal > 0 ? (
        hasPartialMemory ? (
          <div
            className="flex items-start gap-2 rounded-md border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-sm text-amber-700 dark:text-amber-400"
            data-testid="agent-snapshot-import-partial-memory"
          >
            <AlertCircle className="mt-0.5 h-4 w-4 shrink-0" />
            <div className="flex flex-col gap-1">
              <p>
                {t("agents.import.memoryPartial", {
                  written: result.memoryWritten,
                  total: result.memoryTotal,
                })}
              </p>
              {result.memoryErrors.length > 0 ? (
                <ul
                  className="mt-1 max-h-32 space-y-0.5 overflow-y-auto text-xs"
                  data-testid="agent-snapshot-import-memory-errors"
                >
                  {result.memoryErrors.map((err) => (
                    <li key={err} className="break-all font-mono">
                      {err}
                    </li>
                  ))}
                </ul>
              ) : null}
            </div>
          </div>
        ) : (
          <p
            className="text-xs text-muted-foreground"
            data-testid="agent-snapshot-import-memory-success"
          >
            {t("agents.import.memoryRestored", { count: result.memoryTotal })}
          </p>
        )
      ) : null}

      {result.profileSyncError ? (
        <p className="text-xs text-amber-600 dark:text-amber-400">
          {t("agents.import.profileSync", { error: result.profileSyncError })}
        </p>
      ) : null}

      {confirmError ? (
        <p className="text-xs text-destructive">{confirmError}</p>
      ) : null}
    </div>
  );
}
