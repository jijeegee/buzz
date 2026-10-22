import { SettingsConfirmDialog } from "@/features/settings/ui/SettingsConfirmDialog";

export const MAKE_PUBLIC_BODY =
  "Anyone on Kuzz will be able to find this group and read its past messages.";
export const MAKE_PRIVATE_BODY =
  "The public address will be removed and the group will disappear from search. Members stay.";

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
  const toPublic = nextVisibility === "open";
  return (
    <SettingsConfirmDialog
      confirmLabel={toPublic ? "Make public" : "Make private"}
      description={toPublic ? MAKE_PUBLIC_BODY : MAKE_PRIVATE_BODY}
      onConfirm={onConfirm}
      onOpenChange={onOpenChange}
      open={open}
      testId="group-visibility-confirm"
      title={toPublic ? "Make this group public?" : "Make this group private?"}
    />
  );
}
