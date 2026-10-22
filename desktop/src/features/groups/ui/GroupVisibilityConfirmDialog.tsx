import { SettingsConfirmDialog } from "@/features/settings/ui/SettingsConfirmDialog";
import { useT } from "@/shared/i18n";

/** Confirm before a group switches between public and private. */
export function GroupVisibilityConfirmDialog({
  nextVisibility,
  onConfirm,
  onOpenChange,
  open,
}: {
  nextVisibility: "open" | "private";
  onConfirm: () => void;
  onOpenChange: (open: boolean) => void;
  open: boolean;
}) {
  const t = useT();
  const toPublic = nextVisibility === "open";
  return (
    <SettingsConfirmDialog
      confirmLabel={
        toPublic
          ? t("groups.visibilityConfirm.makePublic")
          : t("groups.visibilityConfirm.makePrivate")
      }
      description={
        toPublic
          ? t("groups.visibilityConfirm.publicBody")
          : t("groups.visibilityConfirm.privateBody")
      }
      onConfirm={onConfirm}
      onOpenChange={onOpenChange}
      open={open}
      testId="group-visibility-confirm"
      title={
        toPublic
          ? t("groups.visibilityConfirm.publicTitle")
          : t("groups.visibilityConfirm.privateTitle")
      }
    />
  );
}
