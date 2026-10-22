import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/shared/ui/alert-dialog";
import { type MessageKey, useT } from "@/shared/i18n";
import { Button } from "@/shared/ui/button";

/** What a forum delete action removes. */
export type ForumDeleteKind = "post" | "reply";

/** Delete button / menu item text per kind. */
export const FORUM_DELETE_ACTION_KEYS: Record<ForumDeleteKind, MessageKey> = {
  post: "forum.delete.postAction",
  reply: "forum.delete.replyAction",
};

const TITLE_KEYS: Record<ForumDeleteKind, MessageKey> = {
  post: "forum.delete.postTitle",
  reply: "forum.delete.replyTitle",
};

const BODY_KEYS: Record<ForumDeleteKind, MessageKey> = {
  post: "forum.delete.postBody",
  reply: "forum.delete.replyBody",
};

export function DeleteConfirmDialog({
  open,
  onOpenChange,
  onConfirm,
  label,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onConfirm: () => void;
  label: ForumDeleteKind;
}) {
  const t = useT();
  return (
    <AlertDialog onOpenChange={onOpenChange} open={open}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{t(TITLE_KEYS[label])}</AlertDialogTitle>
          <AlertDialogDescription>{t(BODY_KEYS[label])}</AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel asChild>
            <Button type="button" variant="outline">
              {t("common.cancel")}
            </Button>
          </AlertDialogCancel>
          <AlertDialogAction asChild>
            <Button onClick={onConfirm} type="button" variant="destructive">
              {t(FORUM_DELETE_ACTION_KEYS[label])}
            </Button>
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
