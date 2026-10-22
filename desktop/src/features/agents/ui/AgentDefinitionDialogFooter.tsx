import { Button } from "@/shared/ui/button";
import { useT } from "@/shared/i18n";

type AgentDefinitionDialogFooterProps = {
  canSubmit: boolean;
  isAvatarUploadPending: boolean;
  isPending: boolean;
  onCancel: () => void;
  publishesCatalogUpdates: boolean;
  submitLabel: string;
};

export function AgentDefinitionDialogFooter({
  canSubmit,
  isAvatarUploadPending,
  isPending,
  onCancel,
  publishesCatalogUpdates,
  submitLabel,
}: AgentDefinitionDialogFooterProps) {
  const t = useT();
  return (
    <div className="flex w-full flex-wrap items-center justify-between gap-3">
      <div className="flex min-h-9 min-w-0 flex-wrap items-center gap-3">
        {publishesCatalogUpdates ? (
          <p
            className="max-w-sm text-xs text-muted-foreground"
            data-testid="persona-dialog-catalog-publish-notice"
          >
            {t("agents.dialog.catalogPublishNotice")}
          </p>
        ) : null}
      </div>

      <div className="flex items-center gap-2">
        <Button
          disabled={isPending || isAvatarUploadPending}
          onClick={onCancel}
          type="button"
          variant="outline"
        >
          {t("common.cancel")}
        </Button>
        <Button
          data-testid="persona-dialog-submit"
          disabled={!canSubmit}
          form="persona-dialog-form"
          type="submit"
        >
          {isPending
            ? t("agents.dialog.saving")
            : isAvatarUploadPending
              ? t("agents.avatarPicker.uploading")
              : publishesCatalogUpdates
                ? t("agents.dialog.saveAndPublish")
                : submitLabel}
        </Button>
      </div>
    </div>
  );
}
