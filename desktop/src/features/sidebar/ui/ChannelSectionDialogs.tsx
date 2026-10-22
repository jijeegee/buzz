import * as React from "react";

import { SmilePlus, X } from "lucide-react";

import { EmojiPicker } from "@/features/custom-emoji/ui/EmojiPicker";
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
import { Button } from "@/shared/ui/button";
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/shared/ui/dialog";
import { StatusEmoji } from "@/features/user-status/ui/StatusEmoji";
import { Input } from "@/shared/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/shared/ui/popover";
import type { Channel } from "@/shared/api/types";
import { useT } from "@/shared/i18n";
import { isGroupChat } from "@/features/channels/lib/groupChat";
import {
  useDeleteChannelMutation,
  useLeaveChannelMutation,
} from "@/features/channels/hooks";
import { ChannelDeleteConfirmationDialog } from "@/features/channels/ui/ChannelManagementModerationActions";
import { useOwnAgentsToRemoveOnLeave } from "@/features/channels/ui/useOwnAgentsToRemoveOnLeave";

export type SectionDialogValue = {
  name: string;
  icon?: string;
};

type SectionNameDialogProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  description: string;
  initialValue: string;
  initialIcon?: string;
  confirmLabel: string;
  isConfirmDisabled: (trimmed: string, icon: string) => boolean;
  onConfirm: (value: SectionDialogValue) => void;
};

function SectionNameDialog({
  open,
  onOpenChange,
  title,
  description,
  initialValue,
  initialIcon = "",
  confirmLabel,
  isConfirmDisabled,
  onConfirm,
}: SectionNameDialogProps) {
  const t = useT();
  const [name, setName] = React.useState(initialValue);
  const [icon, setIcon] = React.useState(initialIcon);
  const [pickerOpen, setPickerOpen] = React.useState(false);
  const inputRef = React.useRef<HTMLInputElement>(null);

  React.useEffect(() => {
    if (!open) {
      setPickerOpen(false);
      return;
    }
    setName(initialValue);
    setIcon(initialIcon);
    // Small delay to let dialog animation start before focusing
    const timerId = globalThis.setTimeout(() => {
      inputRef.current?.focus();
    }, 50);
    return () => globalThis.clearTimeout(timerId);
  }, [open, initialValue, initialIcon]);

  function handleIconSelect(selectedIcon: string) {
    setIcon(selectedIcon);
    setPickerOpen(false);
  }

  function handleSubmit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const trimmed = name.trim();
    const trimmedIcon = icon.trim();
    if (isConfirmDisabled(trimmed, trimmedIcon)) return;
    onConfirm({
      name: trimmed,
      ...(trimmedIcon ? { icon: trimmedIcon } : {}),
    });
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-sm">
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          <DialogDescription>{description}</DialogDescription>
        </DialogHeader>
        <form onSubmit={handleSubmit}>
          <div className="flex items-center gap-2">
            <Popover onOpenChange={setPickerOpen} open={pickerOpen}>
              <div className="relative shrink-0">
                <PopoverTrigger asChild>
                  <button
                    aria-label={t("sidebar.sectionDialog.chooseIcon")}
                    className="flex h-9 w-9 items-center justify-center rounded-md border border-input text-lg transition-colors hover:bg-accent"
                    type="button"
                  >
                    {icon ? (
                      <StatusEmoji className="h-5 w-5" value={icon} />
                    ) : (
                      <SmilePlus className="h-4 w-4 text-muted-foreground" />
                    )}
                  </button>
                </PopoverTrigger>
                {icon ? (
                  <button
                    aria-label={t("sidebar.sectionDialog.clearIcon")}
                    className="absolute -right-1 -top-1 flex h-4 w-4 items-center justify-center rounded-full border border-background bg-muted text-muted-foreground hover:bg-accent hover:text-foreground"
                    onClick={(event) => {
                      event.stopPropagation();
                      setIcon("");
                    }}
                    type="button"
                  >
                    <X className="h-3 w-3" />
                  </button>
                ) : null}
              </div>
              <PopoverContent
                align="start"
                className="w-auto overflow-hidden rounded-2xl p-0"
                sideOffset={4}
              >
                <EmojiPicker onSelect={handleIconSelect} />
              </PopoverContent>
            </Popover>
            <Input
              autoCapitalize="none"
              autoComplete="off"
              autoCorrect="off"
              className="flex-1"
              onChange={(event) => setName(event.target.value)}
              placeholder={t("sidebar.sectionDialog.namePlaceholder")}
              ref={inputRef}
              spellCheck={false}
              value={name}
            />
          </div>
          <div className="flex justify-end gap-2 mt-4">
            <DialogClose asChild>
              <Button variant="ghost" type="button">
                {t("common.cancel")}
              </Button>
            </DialogClose>
            <Button
              type="submit"
              disabled={isConfirmDisabled(name.trim(), icon.trim())}
            >
              {confirmLabel}
            </Button>
          </div>
        </form>
      </DialogContent>
    </Dialog>
  );
}

export type CreateSectionDialogProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onConfirm: (value: SectionDialogValue) => void;
};

export function CreateSectionDialog({
  open,
  onOpenChange,
  onConfirm,
}: CreateSectionDialogProps) {
  const t = useT();
  return (
    <SectionNameDialog
      open={open}
      onOpenChange={onOpenChange}
      title={t("sidebar.sectionDialog.createTitle")}
      description={t("sidebar.sectionDialog.createDescription")}
      initialValue=""
      confirmLabel={t("common.create")}
      isConfirmDisabled={(trimmed) => trimmed.length === 0}
      onConfirm={onConfirm}
    />
  );
}

export type RenameSectionDialogProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  sectionName: string;
  sectionIcon?: string;
  onConfirm: (value: SectionDialogValue) => void;
};

export function RenameSectionDialog({
  open,
  onOpenChange,
  sectionName,
  sectionIcon,
  onConfirm,
}: RenameSectionDialogProps) {
  const t = useT();
  return (
    <SectionNameDialog
      open={open}
      onOpenChange={onOpenChange}
      title={t("sidebar.sectionDialog.renameTitle")}
      description={t("sidebar.sectionDialog.renameDescription")}
      initialValue={sectionName}
      initialIcon={sectionIcon}
      confirmLabel={t("common.save")}
      isConfirmDisabled={(trimmed, icon) =>
        trimmed.length === 0 ||
        (trimmed === sectionName && icon === (sectionIcon ?? ""))
      }
      onConfirm={onConfirm}
    />
  );
}

export type DeleteSectionAlertDialogProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  sectionName: string;
  channelCount: number;
  onConfirm: () => void;
};

export function DeleteSectionAlertDialog({
  open,
  onOpenChange,
  sectionName,
  channelCount,
  onConfirm,
}: DeleteSectionAlertDialogProps) {
  const t = useT();
  const description = t("sidebar.deleteSection.description", {
    count: channelCount,
    name: sectionName,
  });

  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>
            {t("sidebar.deleteSection.title")}
          </AlertDialogTitle>
          <AlertDialogDescription>{description}</AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>{t("common.cancel")}</AlertDialogCancel>
          <AlertDialogAction
            className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
            onClick={onConfirm}
          >
            {t("common.delete")}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}

// ---------------------------------------------------------------------------
// LeaveChannelAlertDialog
// ---------------------------------------------------------------------------

export type LeaveChannelAlertDialogProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  channelName: string;
  /** A group DM: no name to quote and no way back in on your own. */
  isConversation?: boolean;
  /** An ownerless chat: leaving removes only you; someone can add you back. */
  isChat?: boolean;
  /**
   * My agents in the room. When non-zero the dialog offers "Leave and remove
   * my agents (N)" (`onConfirm(true)`) next to "Leave only" (`onConfirm(false)`).
   */
  ownAgentCount?: number;
  onConfirm: (removeOwnAgents: boolean) => void;
};

export function LeaveChannelAlertDialog({
  open,
  onOpenChange,
  channelName,
  isConversation = false,
  isChat = false,
  ownAgentCount = 0,
  onConfirm,
}: LeaveChannelAlertDialogProps) {
  const t = useT();
  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>
            {isChat
              ? t("sidebar.leaveDialog.chatTitle")
              : isConversation
                ? t("sidebar.leaveDialog.conversationTitle")
                : t("sidebar.leaveDialog.groupTitle")}
          </AlertDialogTitle>
          <AlertDialogDescription>
            {isChat
              ? t("sidebar.leaveDialog.chatDescription")
              : isConversation
                ? t("sidebar.leaveDialog.conversationDescription")
                : t("sidebar.leaveDialog.groupDescription", {
                    name: channelName,
                  })}
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>{t("common.cancel")}</AlertDialogCancel>
          {ownAgentCount > 0 ? (
            <>
              <AlertDialogAction
                data-testid="leave-channel-only"
                onClick={() => onConfirm(false)}
              >
                {t("sidebar.leaveDialog.leaveOnly")}
              </AlertDialogAction>
              <AlertDialogAction
                className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
                data-testid="leave-channel-remove-agents"
                onClick={() => onConfirm(true)}
              >
                {t("sidebar.leaveDialog.leaveWithAgents", {
                  count: ownAgentCount,
                })}
              </AlertDialogAction>
            </>
          ) : (
            <AlertDialogAction
              className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
              onClick={() => onConfirm(false)}
            >
              {t("sidebar.leaveDialog.confirm")}
            </AlertDialogAction>
          )}
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}

// ---------------------------------------------------------------------------
// useLeaveChannelDialog — owns leave-channel state, mutation, and dialog
// ---------------------------------------------------------------------------

export function useLeaveChannelDialog() {
  const [target, setTarget] = React.useState<Channel | null>(null);
  const leaveChannel = useLeaveChannelMutation(target?.id ?? null);
  const ownAgents = useOwnAgentsToRemoveOnLeave(
    target?.id ?? null,
    target !== null,
  );

  const dialog = (
    <LeaveChannelAlertDialog
      open={target !== null}
      onOpenChange={(open) => {
        if (!open) setTarget(null);
      }}
      channelName={target?.name ?? ""}
      isChat={isGroupChat(target)}
      isConversation={target?.channelType === "dm"}
      onConfirm={(removeOwnAgents) => {
        if (target) {
          leaveChannel.mutate({
            removeAgentPubkeys: removeOwnAgents ? ownAgents : [],
          });
        }
        setTarget(null);
      }}
      ownAgentCount={target?.channelType === "dm" ? 0 : ownAgents.length}
    />
  );

  return { requestLeaveChannel: setTarget, dialog };
}

export function useDeleteChannelDialog(onDeleted: (channel: Channel) => void) {
  const [target, setTarget] = React.useState<Channel | null>(null);
  const deleteChannel = useDeleteChannelMutation(target?.id ?? null);

  const dialog = (
    <ChannelDeleteConfirmationDialog
      channelName={target?.name ?? ""}
      error={deleteChannel.error}
      isPending={deleteChannel.isPending}
      onConfirm={() => {
        const deletedChannel = target;
        deleteChannel.mutate(undefined, {
          onSuccess: () => {
            setTarget(null);
            if (deletedChannel) onDeleted(deletedChannel);
          },
        });
      }}
      onOpenChange={(open) => {
        if (!open) {
          deleteChannel.reset();
          setTarget(null);
        }
      }}
      open={target !== null}
    />
  );

  return { requestDeleteChannel: setTarget, dialog };
}
