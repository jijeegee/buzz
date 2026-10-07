import { useState, type ComponentProps } from "react";
import { MessageThreadPanelHeader } from "./MessageThreadPanelSkeleton";
import { Pencil } from "lucide-react";
import { useThreadName } from "../lib/useThreadName";
import { isValidThreadName, threadNameWeight } from "../lib/threadName";
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
        headerActions={
          <>
            {error ? (
              <Button size="sm" variant="ghost" onClick={retry}>
                Retry name
              </Button>
            ) : null}
            <Button
              size="icon"
              variant="ghost"
              aria-label={name ? "Rename thread" : "Set thread name"}
              disabled={disabled}
              onClick={() => {
                setDraft(name);
                setSaveError("");
                setOpen(true);
              }}
            >
              <Pencil className="size-4" />
            </Button>
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
                    : "Could not save thread name",
                );
              } finally {
                setSaving(false);
              }
            }}
          >
            <DialogHeader>
              <DialogTitle>Thread name</DialogTitle>
              <DialogDescription>
                Visible to everyone in this channel. Up to 40 English or 20
                Korean characters. Leave empty to remove the name.
              </DialogDescription>
            </DialogHeader>
            <Input
              className="mt-4"
              aria-label="Thread name"
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
              {threadNameWeight(normalized)} / 40 · English 1, Korean 2
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
                Cancel
              </Button>
              <Button
                type="submit"
                disabled={saving || !isValidThreadName(normalized)}
              >
                {saving ? "Saving…" : "Save"}
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>
    </>
  );
}
