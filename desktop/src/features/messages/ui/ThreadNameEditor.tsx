import { useT } from "@/shared/i18n";
import { useState, type ComponentProps } from "react";
import { MessageThreadPanelHeader } from "./MessageThreadPanelSkeleton";
import { Pencil } from "lucide-react";
import { useThreadName } from "../lib/useThreadName";
import {
  isValidThreadName,
  MAX_THREAD_NAME_WEIGHT,
  threadNameWeight,
} from "../lib/threadName";
import { Button } from "@/shared/ui/button";
import { Input } from "@/shared/ui/input";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogFooter,
} from "@/shared/ui/dialog";

export function ThreadNameEditor({
  channelId,
  threadId,
  disabled,
  headerProps,
}: {
  channelId: string;
  threadId: string;
  disabled?: boolean;
  headerProps: ComponentProps<typeof MessageThreadPanelHeader>;
}) {
  const t = useT();
  const { name, error, retry, save } = useThreadName(channelId, threadId);
  const [open, setOpen] = useState(false);
  const [draft, setDraft] = useState("");
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState("");
  const normalized = draft.normalize("NFC").trim();
  return (
    <>
      <MessageThreadPanelHeader
        {...headerProps}
        headerTitle={name || headerProps.headerTitle}
        headerTitleAccessory={
          <>
            <Button
              className="shrink-0 text-muted-foreground hover:text-foreground"
              size="icon-xs"
              variant="ghost"
              aria-label={
                name
                  ? t("messages.threadName.rename")
                  : t("messages.threadName.set")
              }
              disabled={disabled}
              onClick={() => {
                setDraft(name);
                setSaveError("");
                setOpen(true);
              }}
            >
              <Pencil />
            </Button>
            {error ? (
              <Button size="sm" variant="ghost" onClick={retry}>
                {t("messages.threadName.retry")}
              </Button>
            ) : null}
          </>
        }
      />
      <Dialog
        open={open}
        onOpenChange={(value) => {
          if (!saving) setOpen(value);
        }}
      >
        <DialogContent>
          <form
            onSubmit={async (event) => {
              event.preventDefault();
              if (saving || !isValidThreadName(normalized)) return;
              setSaving(true);
              setSaveError("");
              try {
                await save(draft);
                setOpen(false);
              } catch (cause) {
                setSaveError(
                  cause instanceof Error
                    ? cause.message
                    : t("messages.threadName.saveFailed"),
                );
              } finally {
                setSaving(false);
              }
            }}
          >
            <DialogHeader>
              <DialogTitle>{t("messages.threadName.title")}</DialogTitle>
              <DialogDescription>
                {t("messages.threadName.description")}
              </DialogDescription>
            </DialogHeader>
            <Input
              className="mt-4"
              aria-label={t("messages.threadName.title")}
              aria-describedby="thread-name-count"
              aria-invalid={!isValidThreadName(normalized)}
              value={draft}
              disabled={saving}
              onChange={(event) => setDraft(event.target.value)}
              onKeyDown={(event) => {
                if (
                  event.key === "Enter" &&
                  (event.nativeEvent.isComposing || event.keyCode === 229)
                )
                  event.preventDefault();
              }}
            />
            <p
              id="thread-name-count"
              className="mt-2 text-xs text-muted-foreground"
            >
              {threadNameWeight(normalized)} / {MAX_THREAD_NAME_WEIGHT}
            </p>
            {saveError ? (
              <p role="alert" className="mt-2 text-sm text-destructive">
                {saveError}
              </p>
            ) : null}
            <DialogFooter className="mt-4">
              <Button
                type="button"
                variant="outline"
                disabled={saving}
                onClick={() => setOpen(false)}
              >
                {t("common.cancel")}
              </Button>
              <Button
                type="submit"
                disabled={saving || !isValidThreadName(normalized)}
              >
                {saving ? t("messages.threadName.saving") : t("common.save")}
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>
    </>
  );
}
