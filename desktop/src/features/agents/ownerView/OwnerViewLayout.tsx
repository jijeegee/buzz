import { useQueryClient } from "@tanstack/react-query";
import * as React from "react";
import { toast } from "sonner";

import { useAppNavigation } from "@/app/navigation/useAppNavigation";
import type { ChatListRow } from "@/features/chats/chatListState";
import { useOpenDmMutation } from "@/features/channels/hooks";
import { useUserProfileQuery } from "@/features/profile/hooks";
import { useT } from "@/shared/i18n";
import { UserAvatar } from "@/shared/ui/UserAvatar";

import { AgentConversationsList } from "./AgentConversationsList";
import { AgentViewTabs } from "./AgentViewTabs";
import { AGENT_CHAT_LIST_QUERY_KEY, ownerViewChatRow } from "./agentChatList";
import { markOwnerViewSeen } from "./ownerViewSeen";
import { useAgentChatList } from "./useAgentChatList";

export type OwnerViewChat = {
  agentName: string;
  agentPubkey: string;
  /** Leave the chat after removing the agent from it. */
  onRemoved: () => void;
  row: ChatListRow;
};

/**
 * "Fizz's conversations": the agent's name and tabs on top, its chats
 * without the owner on the left, the open chat (read-only) on the right.
 */
export function OwnerViewLayout({
  agentPubkey,
  channelId,
  openRootId,
  renderChat,
}: {
  agentPubkey: string;
  channelId: string;
  openRootId: string | null;
  renderChat: (chat: OwnerViewChat) => React.ReactNode;
}) {
  const t = useT();
  const queryClient = useQueryClient();
  const { goChannel } = useAppNavigation();
  const openDm = useOpenDmMutation().mutateAsync;
  const agent = agentPubkey.toLowerCase();
  const profile = useUserProfileQuery(agent).data;
  const agentName =
    profile?.displayName?.trim() || t("ownerView.agentFallback");
  const { error, ownerPubkey, rows } = useAgentChatList(agent);
  const row = rows ? ownerViewChatRow(rows, channelId) : null;

  // Opening a chat or thread clears its dot, and keeps it clear while open.
  React.useEffect(() => {
    if (!ownerPubkey || !rows) return;
    for (const candidate of rows) {
      if (candidate.channel_id !== channelId || !candidate.latest) continue;
      const isOpen =
        candidate.type === "room" || candidate.root_id === openRootId;
      if (!isOpen) continue;
      markOwnerViewSeen(
        ownerPubkey,
        agent,
        candidate.type === "room"
          ? candidate.channel_id
          : `thread:${candidate.root_id}`,
        candidate.latest.created_at,
      );
    }
  }, [agent, channelId, openRootId, ownerPubkey, rows]);

  const openChat = React.useCallback(
    (id: string) => void goChannel(id, { viewAs: agent }),
    [agent, goChannel],
  );
  const openThread = React.useCallback(
    (id: string, rootId: string) =>
      void goChannel(id, { thread: rootId, viewAs: agent }),
    [agent, goChannel],
  );
  const onRemoved = React.useCallback(() => {
    void queryClient.invalidateQueries({
      queryKey: AGENT_CHAT_LIST_QUERY_KEY,
    });
    openDm({ pubkeys: [agent] })
      .then((dm) => goChannel(dm.id, { replace: true, viewAs: agent }))
      .catch((cause: unknown) => {
        toast.error(
          cause instanceof Error
            ? cause.message
            : t("agents.list.openDmFailed"),
        );
      });
  }, [agent, goChannel, openDm, queryClient, t]);

  const chat = React.useMemo<OwnerViewChat | null>(
    () => (row ? { agentName, agentPubkey: agent, onRemoved, row } : null),
    [agent, agentName, onRemoved, row],
  );

  return (
    <div
      className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden"
      data-testid="owner-view-layout"
    >
      <div
        className="flex h-[52px] shrink-0 items-center gap-2 border-b border-border/70 px-5"
        data-tauri-drag-region
      >
        <UserAvatar
          accent
          avatarUrl={profile?.avatarUrl ?? null}
          className="h-7 w-7 text-xs"
          displayName={agentName}
          shape="squircle"
          size="sm"
        />
        <span className="truncate text-base font-semibold">{agentName}</span>
        <AgentViewTabs
          active="conversations"
          agentName={agentName}
          agentPubkey={agent}
        />
      </div>
      <div className="flex min-h-0 min-w-0 flex-1 overflow-hidden">
        <AgentConversationsList
          agentName={agentName}
          agentPubkey={agent}
          onOpenChat={openChat}
          onOpenThread={openThread}
          openRootId={openRootId}
          rows={rows}
          selectedChannelId={row ? channelId : null}
        />
        <div className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden bg-muted/30">
          {chat ? (
            renderChat(chat)
          ) : (
            <div
              className="flex flex-1 items-center justify-center px-6 text-center text-sm text-muted-foreground"
              data-testid="owner-view-placeholder"
            >
              {error
                ? t("ownerView.loadFailed")
                : rows === undefined
                  ? t("ownerView.loading")
                  : t("ownerView.pickChat")}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
