import * as React from "react";
import { toast } from "sonner";

import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/shared/ui/alert-dialog";
import { Button } from "@/shared/ui/button";

/** Toast text after adding a friend. */
export function friendAddedMessage(name: string): string {
  return `${name} is now your friend`;
}

/** Toast text after removing a friend. */
export function friendRemovedMessage(name: string): string {
  return `${name} was removed from your friends`;
}

function errorText(error: unknown, fallback: string): string {
  return error instanceof Error && error.message ? error.message : fallback;
}

/** Adds a friend and says so (or why it failed) in a toast. */
export function addFriendWithToast(
  addFriend: (pubkey: string) => Promise<unknown>,
  pubkey: string,
  name: string,
): Promise<void> {
  return addFriend(pubkey).then(
    () => {
      toast.success(friendAddedMessage(name));
    },
    (error: unknown) => {
      toast.error(errorText(error, "Could not add friend"));
    },
  );
}

export type RemoveFriendTarget = { pubkey: string; name: string };

/**
 * Asks before removing a friend, then confirms the removal in a toast.
 * Open while `target` is set; closing clears it.
 */
export function RemoveFriendDialog({
  onClose,
  onRemoveFriend,
  target,
}: {
  onClose: () => void;
  onRemoveFriend: (pubkey: string) => Promise<unknown>;
  target: RemoveFriendTarget | null;
}) {
  const [pending, setPending] = React.useState(false);
  // Keep the name while the dialog animates closed.
  const lastTarget = React.useRef(target);
  if (target) lastTarget.current = target;
  const name = lastTarget.current?.name ?? "";

  const remove = () => {
    if (!target || pending) return;
    setPending(true);
    onRemoveFriend(target.pubkey)
      .then(
        () => {
          toast.success(friendRemovedMessage(target.name));
        },
        (error: unknown) => {
          toast.error(errorText(error, "Could not remove friend"));
        },
      )
      .finally(() => {
        setPending(false);
        onClose();
      });
  };

  return (
    <AlertDialog
      onOpenChange={(open) => {
        if (!open && !pending) onClose();
      }}
      open={target !== null}
    >
      <AlertDialogContent data-testid="remove-friend-dialog">
        <AlertDialogHeader>
          <AlertDialogTitle>Remove {name} from friends?</AlertDialogTitle>
          <AlertDialogDescription>
            You can add them again anytime.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel asChild>
            <Button disabled={pending} type="button" variant="outline">
              Cancel
            </Button>
          </AlertDialogCancel>
          <Button
            data-testid="remove-friend-confirm"
            disabled={pending}
            onClick={remove}
            type="button"
            variant="destructive"
          >
            Remove
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
