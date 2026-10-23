import { Eye, LogOut, Quote } from "lucide-react";
import * as React from "react";
import { toast } from "sonner";

import { useAppNavigation } from "@/app/navigation/useAppNavigation";
import {
  useOpenDmMutation,
  useRemoveChannelMemberMutation,
} from "@/features/channels/hooks";
import {
  loadDraftEntry,
  saveDraftEntry,
} from "@/features/messages/lib/useDrafts";
import { useT } from "@/shared/i18n";
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

/**
 * In place of the composer in the owner view: nothing can be written here.
 * Ask opens the DM with the agent with a draft about this chat; Remove takes
 * the agent out of the chat (the relay lets an agent's owner do that).
 */
export function OwnerViewBar({
  agentName,
  agentPubkey,
  chatLabel,
  channelId,
  onRemoved,
}: {
  agentName: string;
  agentPubkey: string;
  chatLabel: string;
  channelId: string;
  onRemoved: () => void;
}) {
  const t = useT();
  const { goChannel } = useAppNavigation();
  const openDmMutation = useOpenDmMutation();
  const removeMutation = useRemoveChannelMemberMutation(channelId);
  const [confirmOpen, setConfirmOpen] = React.useState(false);

  const ask = async () => {
    try {
      const dm = await openDmMutation.mutateAsync({ pubkeys: [agentPubkey] });
      const now = new Date().toISOString();
      const existing = loadDraftEntry(dm.id);
      const prompt = t("ownerView.askDraft", { chat: chatLabel, channelId });
      const content = existing?.content?.trim()
        ? `${existing.content.trimEnd()}\n\n${prompt}`
        : prompt;
      saveDraftEntry(dm.id, {
        channelId: dm.id,
        content,
        createdAt: existing?.createdAt ?? now,
        mentionRefs: existing?.mentionRefs ?? [],
        pendingImeta: existing?.pendingImeta ?? [],
        selectionEnd: content.length,
        selectionStart: content.length,
        spoileredAttachmentUrls: existing?.spoileredAttachmentUrls ?? [],
        status: "active",
        updatedAt: now,
      });
      await goChannel(dm.id);
    } catch (error) {
      toast.error(
        error instanceof Error ? error.message : t("agents.list.openDmFailed"),
      );
    }
  };

  const remove = async () => {
    try {
      await removeMutation.mutateAsync(agentPubkey);
      toast.success(t("ownerView.removed", { name: agentName }));
      onRemoved();
    } catch (error) {
      toast.error(
        error instanceof Error
          ? error.message
          : t("ownerView.removeFailed", { name: agentName }),
      );
    }
  };

  return (
    <div className="flex items-center gap-3 px-5 py-3">
      <Eye
        aria-hidden="true"
        className="h-4 w-4 shrink-0 text-muted-foreground"
      />
      <p className="min-w-0 flex-1 text-sm text-muted-foreground">
        {t("ownerView.bar.notice", { name: agentName })}
      </p>
      <Button
        data-testid="owner-view-ask"
        disabled={openDmMutation.isPending}
        onClick={() => void ask()}
        size="sm"
        variant="outline"
      >
        <Quote className="mr-1.5 h-3.5 w-3.5" />
        {t("ownerView.bar.ask", { name: agentName })}
      </Button>
      <Button
        className="text-destructive hover:text-destructive"
        data-testid="owner-view-remove"
        disabled={removeMutation.isPending}
        onClick={() => setConfirmOpen(true)}
        size="sm"
        variant="outline"
      >
        <LogOut className="mr-1.5 h-3.5 w-3.5" />
        {t("ownerView.bar.remove", { name: agentName })}
      </Button>
      <AlertDialog onOpenChange={setConfirmOpen} open={confirmOpen}>
        <AlertDialogContent data-testid="owner-view-remove-dialog">
          <AlertDialogHeader>
            <AlertDialogTitle>
              {t("ownerView.removeConfirm.title", {
                chat: chatLabel,
                name: agentName,
              })}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {t("ownerView.removeConfirm.body", { name: agentName })}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t("common.cancel")}</AlertDialogCancel>
            <AlertDialogAction
              data-testid="owner-view-remove-confirm"
              onClick={() => void remove()}
            >
              {t("ownerView.removeConfirm.action")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
