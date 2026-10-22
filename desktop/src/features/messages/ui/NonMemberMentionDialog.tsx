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
import { PRIVATE_CHANNEL_ADD_DENIED_MESSAGE } from "@/features/channels/lib/channelMemberAdmission";

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
              ? `Add ${names.join(", ")} to this chat?`
              : "Mention people outside this group?"}
          </AlertDialogTitle>
          <AlertDialogDescription>
            {isChat ? (
              `${names.join(", ")} ${names.length === 1 ? "isn't" : "aren't"} in this chat.${
                onDoNothing ? " You can also send without adding them." : ""
              }`
            ) : (
              <>
                {names.join(", ")} {names.length === 1 ? "is" : "are"} not in
                this group.{" "}
              </>
            )}
            {isChat
              ? null
              : onDoNothing
                ? canInvite
                  ? "Add them to the group, or send without inviting them."
                  : `${PRIVATE_CHANNEL_ADD_DENIED_MESSAGE} You can still send without inviting them.`
                : canInvite
                  ? "Add them to the group, or cancel to keep your draft."
                  : PRIVATE_CHANNEL_ADD_DENIED_MESSAGE}
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
                  ? "Send without adding"
                  : "Do nothing"
                : "Send anyway"
              : "Cancel"}
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
                  ? "Adding..."
                  : "Add"
                : isInvitePending
                  ? "Inviting..."
                  : "Invite"}
            </Button>
          ) : null}
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
