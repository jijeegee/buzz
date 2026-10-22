import type { Workflow } from "@/shared/api/types";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/shared/ui/alert-dialog";
import { useT } from "@/shared/i18n";
import { Button } from "@/shared/ui/button";

type WorkflowDeleteDialogProps = {
  error?: string | null;
  isPending?: boolean;
  open: boolean;
  workflow: Workflow | null;
  onConfirm: (workflow: Workflow) => Promise<void>;
  onOpenChange: (open: boolean) => void;
};

export function WorkflowDeleteDialog({
  error,
  isPending = false,
  open,
  workflow,
  onConfirm,
  onOpenChange,
}: WorkflowDeleteDialogProps) {
  const t = useT();
  return (
    <AlertDialog
      onOpenChange={(nextOpen) => {
        if (!isPending) onOpenChange(nextOpen);
      }}
      open={open}
    >
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>
            {t("workflows.deleteDialog.title")}
          </AlertDialogTitle>
          <AlertDialogDescription>
            {workflow
              ? t("workflows.deleteDialog.description", { name: workflow.name })
              : t("workflows.deleteDialog.descriptionUnnamed")}
          </AlertDialogDescription>
          {error ? (
            <p
              aria-live="polite"
              className="rounded-xl border border-destructive/30 bg-destructive/10 px-3 py-2 text-sm text-destructive"
              role="alert"
            >
              {t("workflows.deleteDialog.failed", { error })}
            </p>
          ) : null}
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel asChild>
            <Button disabled={isPending} type="button" variant="outline">
              {t("common.cancel")}
            </Button>
          </AlertDialogCancel>
          <Button
            disabled={!workflow || isPending}
            onClick={() => {
              if (workflow) {
                void onConfirm(workflow);
              }
            }}
            type="button"
            variant="destructive"
          >
            {isPending
              ? t("workflows.deleteDialog.deleting")
              : t("common.delete")}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
