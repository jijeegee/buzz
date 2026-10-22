import * as React from "react";
import { Ban, Flag, MoreHorizontal, ShieldCheck } from "lucide-react";
import { toast } from "sonner";

import { ReportMessageDialog } from "@/features/moderation/ui/ReportMessageDialog";
import {
  BLOCKED_BUT_STILL_FRIEND,
  useBlockedPeople,
  useBlockPersonMutation,
  useUnblockPersonMutation,
} from "@/features/people/BlockedPeopleContext";
import { useIdentityQuery } from "@/shared/api/hooks";
import { t as translate, useT } from "@/shared/i18n";
import { cn } from "@/shared/lib/cn";
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
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/shared/ui/dropdown-menu";

/** What blocking does, in the words the confirm dialog uses. */
export function blockDescription(name: string): string {
  return translate("people.safety.blockDescription", { name });
}

/**
 * "More" menu on a person's profile: Block (or Unblock) and Report. Block
 * asks first; Report opens the report dialog ("Also block" starts off).
 */
export function PersonSafetyMenu({
  className,
  name,
  pubkey,
  variant = "icon",
}: {
  className?: string;
  name: string;
  pubkey: string;
  /** `icon`: a small "…" button; `button`: an outlined "More" button. */
  variant?: "icon" | "button";
}) {
  const t = useT();
  const { isBlocked } = useBlockedPeople();
  const identityQuery = useIdentityQuery();
  const blockPerson = useBlockPersonMutation(identityQuery.data?.pubkey);
  const unblockPerson = useUnblockPersonMutation(identityQuery.data?.pubkey);
  const [confirmOpen, setConfirmOpen] = React.useState(false);
  const [reportOpen, setReportOpen] = React.useState(false);
  const blocked = isBlocked(pubkey);
  const pending = blockPerson.isPending || unblockPerson.isPending;

  const block = () => {
    blockPerson.mutate(pubkey, {
      onSuccess: () => toast.success(t("people.safety.blockedToast", { name })),
      onError: (error) =>
        toast.error(
          error instanceof Error && error.message === BLOCKED_BUT_STILL_FRIEND
            ? t("people.safety.blockedStillFriend")
            : error instanceof Error
              ? error.message
              : t("people.safety.blockFailed"),
        ),
      onSettled: () => setConfirmOpen(false),
    });
  };
  const unblock = () => {
    unblockPerson.mutate(pubkey, {
      onSuccess: () =>
        toast.success(t("people.safety.unblockedToast", { name })),
      onError: (error) =>
        toast.error(
          error instanceof Error
            ? error.message
            : t("people.safety.unblockFailed"),
        ),
    });
  };

  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          {variant === "icon" ? (
            <Button
              aria-label={t("people.safety.moreOptions", { name })}
              className={cn("text-muted-foreground", className)}
              data-testid="person-safety-menu"
              disabled={pending}
              size="icon"
              title={t("common.more")}
              variant="ghost"
            >
              <MoreHorizontal className="h-4 w-4" />
            </Button>
          ) : (
            <Button
              aria-label={t("people.safety.moreOptions", { name })}
              className={className}
              data-testid="person-safety-menu"
              disabled={pending}
              variant="outline"
            >
              <MoreHorizontal className="h-4 w-4" />
              {t("common.more")}
            </Button>
          )}
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          {blocked ? (
            <DropdownMenuItem
              data-testid="person-unblock"
              onSelect={() => unblock()}
            >
              <ShieldCheck className="h-4 w-4" />
              {t("people.safety.unblock")}
            </DropdownMenuItem>
          ) : (
            <DropdownMenuItem
              data-testid="person-block"
              onSelect={() => setConfirmOpen(true)}
            >
              <Ban className="h-4 w-4" />
              {t("people.safety.block")}
            </DropdownMenuItem>
          )}
          <DropdownMenuItem
            data-testid="person-report"
            onSelect={() => setReportOpen(true)}
          >
            <Flag className="h-4 w-4" />
            {t("people.safety.report")}
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>

      <AlertDialog onOpenChange={setConfirmOpen} open={confirmOpen}>
        <AlertDialogContent data-testid="block-person-dialog">
          <AlertDialogHeader>
            <AlertDialogTitle>
              {t("people.safety.blockConfirmTitle", { name })}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {blockDescription(name)}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel asChild>
              <Button disabled={pending} type="button" variant="outline">
                {t("common.cancel")}
              </Button>
            </AlertDialogCancel>
            <Button
              data-testid="block-person-confirm"
              disabled={pending}
              onClick={block}
              type="button"
              variant="destructive"
            >
              {t("people.safety.blockConfirm")}
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      <ReportMessageDialog
        authorPubkey={pubkey}
        onOpenChange={setReportOpen}
        open={reportOpen}
      />
    </>
  );
}
