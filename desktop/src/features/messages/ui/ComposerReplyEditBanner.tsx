import { CornerUpLeft, Pencil, Quote, X } from "lucide-react";

import type { QuoteTarget } from "@/features/messages/lib/messageQuote";
import { cn } from "@/shared/lib/cn";
import { Button } from "@/shared/ui/button";

const BANNER_CLASS =
  "relative z-0 -mb-4 flex transform-gpu gap-2 rounded-t-2xl border border-b-0 border-border/60 bg-muted/55 px-4 pb-6 pt-2.5 text-sm leading-5 text-muted-foreground backdrop-blur-sm transition-colors";

const CANCEL_BUTTON_CLASS =
  "-mr-1 h-7 w-7 shrink-0 px-0 text-muted-foreground hover:text-foreground";

/** "Quoting …" row: the message the next send will carry as a NIP-18 quote. */
function QuoteTargetRow({
  onCancel,
  target,
}: {
  onCancel?: () => void;
  target: QuoteTarget;
}) {
  return (
    <div className="flex items-start gap-2" data-testid="quote-target">
      <Quote aria-hidden className="mt-0.5 h-4 w-4 shrink-0" />
      <div className="min-w-0 flex-1">
        <p className="truncate font-medium text-foreground">
          Quoting {target.author}
        </p>
        <p className="truncate text-muted-foreground/80">{target.excerpt}</p>
      </div>
      {onCancel ? (
        <Button
          aria-label="Cancel quote"
          className={CANCEL_BUTTON_CLASS}
          onClick={onCancel}
          size="icon"
          type="button"
          variant="ghost"
        >
          <X className="h-4 w-4" />
        </Button>
      ) : null}
    </div>
  );
}

/**
 * The "Editing message" / "Replying to …" / "Quoting …" banner that sits
 * above the composer input. Edit takes precedence over reply and quote
 * (matching the composer's own `editTarget ? … : replyTarget ? …` ordering);
 * a reply target and a quote can show together. Rendered as a sibling so
 * MessageComposer stays under the file-size guard; purely presentational.
 */
export function ComposerReplyEditBanner({
  isEditing,
  isEditCancelDisabled = false,
  quoteTarget = null,
  replyTarget,
  onCancelEdit,
  onCancelQuote,
  onCancelReply,
}: {
  isEditing: boolean;
  isEditCancelDisabled?: boolean;
  quoteTarget?: QuoteTarget | null;
  replyTarget?: { author: string; body: string; id: string } | null;
  onCancelEdit?: () => void;
  onCancelQuote?: () => void;
  onCancelReply?: () => void;
}) {
  if (isEditing) {
    return (
      <div
        className={cn(BANNER_CLASS, "items-center")}
        data-testid="edit-target"
      >
        <Pencil aria-hidden className="h-4 w-4 shrink-0" />
        <div className="min-w-0 flex-1">
          <p className="truncate font-medium text-foreground">
            Editing message
          </p>
        </div>
        {onCancelEdit ? (
          <Button
            aria-label="Cancel edit"
            className="-mr-1 h-7 w-7 shrink-0 px-0 text-muted-foreground hover:text-foreground"
            disabled={isEditCancelDisabled}
            onClick={onCancelEdit}
            size="icon"
            type="button"
            variant="ghost"
          >
            <X className="h-4 w-4" />
          </Button>
        ) : null}
      </div>
    );
  }

  if (replyTarget && quoteTarget) {
    return (
      <div className={cn(BANNER_CLASS, "flex-col")}>
        <div className="flex items-start gap-2" data-testid="reply-target">
          <CornerUpLeft aria-hidden className="mt-0.5 h-4 w-4 shrink-0" />
          <p className="min-w-0 flex-1 truncate font-medium text-foreground">
            Replying to {replyTarget.author}
          </p>
          {onCancelReply ? (
            <Button
              aria-label="Cancel reply"
              className={CANCEL_BUTTON_CLASS}
              onClick={onCancelReply}
              size="icon"
              type="button"
              variant="ghost"
            >
              <X className="h-4 w-4" />
            </Button>
          ) : null}
        </div>
        <QuoteTargetRow onCancel={onCancelQuote} target={quoteTarget} />
      </div>
    );
  }

  if (quoteTarget) {
    return (
      <div className={BANNER_CLASS}>
        <div className="min-w-0 flex-1">
          <QuoteTargetRow onCancel={onCancelQuote} target={quoteTarget} />
        </div>
      </div>
    );
  }

  if (replyTarget) {
    return (
      <div
        className={cn(BANNER_CLASS, "items-start")}
        data-testid="reply-target"
      >
        <CornerUpLeft aria-hidden className="mt-0.5 h-4 w-4 shrink-0" />
        <div className="min-w-0 flex-1">
          <p className="truncate font-medium text-foreground">
            Replying to {replyTarget.author}
          </p>
          {replyTarget.body ? (
            <p className="truncate text-muted-foreground/80">
              {replyTarget.body}
            </p>
          ) : null}
        </div>
        {onCancelReply ? (
          <Button
            aria-label="Cancel reply"
            className="-mr-1 h-7 w-7 shrink-0 px-0 text-muted-foreground hover:text-foreground"
            onClick={onCancelReply}
            size="icon"
            type="button"
            variant="ghost"
          >
            <X className="h-4 w-4" />
          </Button>
        ) : null}
      </div>
    );
  }

  return null;
}
