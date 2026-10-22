import { Flag } from "lucide-react";
import * as React from "react";
import { toast } from "sonner";

import { useSubmitReportMutation } from "@/features/moderation/hooks";
import {
  BLOCKED_BUT_STILL_FRIEND,
  useBlockPersonMutation,
} from "@/features/people/BlockedPeopleContext";
import { useIdentityQuery } from "@/shared/api/hooks";
import { type MessageKey, t as translate, useT } from "@/shared/i18n";
import type { ReportType } from "@/features/moderation/hooks";
import { Button } from "@/shared/ui/button";
import { Checkbox } from "@/shared/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/shared/ui/dialog";
import { Textarea } from "@/shared/ui/textarea";

/** NIP-56 report categories, in the order shown to the reporter. `other` is
 *  last so it reads as the fallback rather than a first-class choice. */
const REPORT_CATEGORIES: { value: ReportType; labelKey: MessageKey }[] = [
  { value: "spam", labelKey: "moderation.report.category.spam" },
  { value: "profanity", labelKey: "moderation.report.category.profanity" },
  { value: "nudity", labelKey: "moderation.report.category.nudity" },
  {
    value: "impersonation",
    labelKey: "moderation.report.category.impersonation",
  },
  { value: "malware", labelKey: "moderation.report.category.malware" },
  { value: "illegal", labelKey: "moderation.report.category.illegal" },
  { value: "other", labelKey: "moderation.report.category.other" },
];

/**
 * Extract a human-readable reason from a report mutation error. Returns the
 * relay's own error message when present, falling back to a generic string.
 *
 * Exported for testing.
 */
export function reportErrorMessage(error: unknown): string {
  if (error instanceof Error) {
    const msg = error.message;
    // Surface the relay's own reason verbatim. Strip any raw status prefix
    // (e.g. "400: ") so the copy reads naturally in a toast.
    const stripped = msg.replace(/^[45]\d\d:\s?/, "").trim();
    return stripped || translate("moderation.report.submitFailed");
  }
  return translate("moderation.report.submitFailed");
}

/** Toast text after a report, with or without the block that came with it. */
export function reportDoneMessage(blocked: boolean): string {
  return blocked
    ? translate("moderation.report.doneBlocked")
    : translate("moderation.report.done");
}

/**
 * Toast text when the report went through but "Also block" did not finish.
 * A block that worked but left them a friend is still a block.
 */
export function reportBlockFailureMessage(error: unknown): string {
  if (error instanceof Error && error.message === BLOCKED_BUT_STILL_FRIEND) {
    return translate("moderation.report.blockedStillFriend");
  }
  return translate("moderation.report.blockFailed");
}

/** "Also block" starts off: a report never blocks unless the user opts in. */
export const ALSO_BLOCK_DEFAULT = false;

/**
 * Report a message (`eventId`) or, without one, a person. "Also block" is off
 * by default; when checked, the report goes first, then the block (which also
 * removes them from friends).
 */
export function ReportMessageDialog({
  open,
  onOpenChange,
  authorPubkey,
  eventId,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** The reported person, or the reported message's author (`p` tag). */
  authorPubkey: string;
  /** Reported message event id (the `e` tag target); none for a person. */
  eventId?: string;
}) {
  const t = useT();
  const submitReport = useSubmitReportMutation();
  const identityQuery = useIdentityQuery();
  const blockPerson = useBlockPersonMutation(identityQuery.data?.pubkey);
  const [category, setCategory] = React.useState<ReportType | null>(null);
  const [note, setNote] = React.useState("");
  const [alsoBlock, setAlsoBlock] = React.useState(ALSO_BLOCK_DEFAULT);
  const isPerson = !eventId;
  const pending = submitReport.isPending || blockPerson.isPending;

  // Reset the form each time the dialog opens so a prior report's selection
  // never leaks into the next one.
  React.useEffect(() => {
    if (open) {
      setCategory(null);
      setNote("");
      setAlsoBlock(ALSO_BLOCK_DEFAULT);
    }
  }, [open]);

  const submit = () => {
    if (!category || pending) return;
    submitReport.mutate(
      {
        authorPubkey,
        eventId,
        reportType: category,
        note: note.trim() || undefined,
      },
      {
        onSuccess: () => {
          if (!alsoBlock) {
            toast.success(reportDoneMessage(false));
            onOpenChange(false);
            return;
          }
          blockPerson.mutate(authorPubkey, {
            onSuccess: () => toast.success(reportDoneMessage(true)),
            onError: (error) => toast.warning(reportBlockFailureMessage(error)),
            onSettled: () => onOpenChange(false),
          });
        },
        onError: (error) => toast.error(reportErrorMessage(error)),
      },
    );
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-[420px]">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <Flag className="h-4 w-4" />
            {isPerson
              ? t("moderation.report.titlePerson")
              : t("moderation.report.titleMessage")}
          </DialogTitle>
          <DialogDescription>
            {isPerson
              ? t("moderation.report.descriptionPerson")
              : t("moderation.report.descriptionMessage")}
          </DialogDescription>
        </DialogHeader>

        <div className="flex flex-col gap-2">
          {REPORT_CATEGORIES.map((item) => (
            <Button
              key={item.value}
              variant={category === item.value ? "default" : "outline"}
              className="justify-start"
              disabled={pending}
              onClick={() => setCategory(item.value)}
            >
              {t(item.labelKey)}
            </Button>
          ))}
        </div>

        <div className="space-y-2">
          <label
            htmlFor="report-note"
            className="text-sm font-medium text-muted-foreground"
          >
            {t("moderation.report.noteLabel")}
          </label>
          <Textarea
            id="report-note"
            placeholder={t("moderation.report.notePlaceholder")}
            value={note}
            onChange={(e) => setNote(e.target.value)}
            rows={3}
            className="resize-none"
          />
        </div>

        <label
          className="flex items-center gap-2 text-sm"
          htmlFor="report-also-block"
        >
          <Checkbox
            checked={alsoBlock}
            data-testid="report-also-block"
            disabled={pending}
            id="report-also-block"
            onCheckedChange={(checked) => setAlsoBlock(checked === true)}
          />
          {t("moderation.report.alsoBlock")}
        </label>

        <DialogFooter>
          <Button
            variant="ghost"
            onClick={() => onOpenChange(false)}
            disabled={pending}
          >
            {t("common.cancel")}
          </Button>
          <Button
            data-testid="report-submit"
            variant="default"
            onClick={submit}
            disabled={!category || pending}
          >
            {t("moderation.report.submit")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
