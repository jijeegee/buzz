import { useT } from "@/shared/i18n";
import * as React from "react";
import {
  AlertDialog,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/shared/ui/alert-dialog";
import { Button } from "@/shared/ui/button";

type NonMemberMentionDialogProps = {
  /** False in a private channel the viewer doesn't own/administer. */
  canInvite: boolean;
  error: string | null;
  isInvitePending: boolean;
  names: string[];
  onDismiss: () => void;
  /** Omit when publication requires the intended recipients to be invited. */
  onDoNothing?: () => void;
  onInvite: () => void;
  open: boolean;
  /** Restore the initiating editor when it still owns the visible draft. */
  onRestoreFocus?: () => void;
  /** Ownerless chat: ask "Add X to this chat?" instead of group wording. */
  isChat?: boolean;
};

export function NonMemberMentionDialog({
  canInvite,
  error,
  isChat = false,
  isInvitePending,
  names,
  onDismiss,
  onDoNothing,
  onInvite,
  open,
  onRestoreFocus,
}: NonMemberMentionDialogProps) {
  const t = useT();
  const nameList = names.join(", ");
  const safeActionRef = React.useRef<HTMLButtonElement>(null);
  const restoreFocusRef = React.useRef(onRestoreFocus);
  return (
    <AlertDialog
      onOpenChange={(nextOpen) => {
        if (!nextOpen) {
          onDismiss();
        }
      }}
      open={open}
    >
      <AlertDialogContent
        onOpenAutoFocus={(event) => {
          event.preventDefault();
          restoreFocusRef.current = onRestoreFocus;
          safeActionRef.current?.focus();
        }}
        onCloseAutoFocus={(event) => {
          if (!restoreFocusRef.current) return;
          event.preventDefault();
          // Pending state/inert is released by the async cancellation continuation.
          // Wait for its React commit; the source checks that it is still visible.
          requestAnimationFrame(() => restoreFocusRef.current?.());
        }}
      >
        <AlertDialogHeader>
          <AlertDialogTitle>
            {isChat
              ? t("messages.nonMember.chatTitle", { names: nameList })
              : t("messages.nonMember.groupTitle")}
          </AlertDialogTitle>
          <AlertDialogDescription>
            {isChat ? (
              `${t("messages.nonMember.chatBody", {
                count: names.length,
                names: nameList,
              })}${onDoNothing ? ` ${t("messages.nonMember.chatSendHint")}` : ""}`
            ) : (
              <>
                {t("messages.nonMember.groupBody", {
                  count: names.length,
                  names: nameList,
                })}{" "}
              </>
            )}
            {isChat
              ? null
              : onDoNothing
                ? canInvite
                  ? t("messages.nonMember.inviteOrSend")
                  : `${t("channels.members.addDenied")} ${t("messages.nonMember.canStillSend")}`
                : canInvite
                  ? t("messages.nonMember.inviteOrCancel")
                  : t("channels.members.addDenied")}
          </AlertDialogDescription>
        </AlertDialogHeader>
        {error ? (
          <p
            role="alert"
            className="rounded-lg bg-destructive/10 px-3 py-2 text-sm text-destructive"
          >
            {error}
          </p>
        ) : null}
        <AlertDialogFooter>
          <Button
            ref={safeActionRef}
            disabled={Boolean(onDoNothing) && isInvitePending}
            onClick={onDoNothing ?? onDismiss}
            size="sm"
            type="button"
            variant="outline"
          >
            {onDoNothing
              ? canInvite
                ? isChat
                  ? t("messages.nonMember.sendWithoutAdding")
                  : t("messages.nonMember.doNothing")
                : t("messages.nonMember.sendAnyway")
              : t("common.cancel")}
          </Button>
          {canInvite ? (
            <Button
              disabled={isInvitePending}
              onClick={onInvite}
              size="sm"
              type="button"
            >
              {isChat
                ? isInvitePending
                  ? t("messages.nonMember.adding")
                  : t("common.add")
                : isInvitePending
                  ? t("messages.nonMember.inviting")
                  : t("messages.nonMember.invite")}
            </Button>
          ) : null}
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
