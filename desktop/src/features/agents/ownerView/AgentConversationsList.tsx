import { Eye } from "lucide-react";
import * as React from "react";

import type { ChatListRow } from "@/features/chats/chatListState";
import type { ChatsViewMode } from "@/features/chats/chatListGroups";
import { isGroupChat } from "@/features/channels/lib/groupChat";
import { useDmSidebarMetadata } from "@/features/sidebar/useDmSidebarMetadata";
import { ChatList, type ServerChats } from "@/features/sidebar/ui/ChatList";
import { ChatsViewToggle } from "@/features/sidebar/ui/ChatsColumnHeader";
import {
  ChatRowDisplayContext,
  type ChatRowDisplay,
} from "@/features/sidebar/ui/chatRowDisplay";
import { useT } from "@/shared/i18n";

import { agentChatChannels } from "./agentChatList";

const NO_COUNTS: ReadonlyMap<string, number> = new Map();
const NO_IDS: ReadonlySet<string> = new Set();
const noReadAt = () => null;

/**
 * The left column of the owner view: the agent's chats without its owner,
 * in the Chats list's own rows (All chats | By group), tighter, with an
 * agent/person tag and dots instead of counts.
 */
export function AgentConversationsList({
  agentName,
  agentPubkey,
  onOpenChat,
  onOpenThread,
  openRootId,
  rows,
  selectedChannelId,
}: {
  agentName: string;
  agentPubkey: string;
  onOpenChat: (channelId: string) => void;
  onOpenThread: (channelId: string, rootId: string) => void;
  openRootId: string | null;
  rows: readonly ChatListRow[] | undefined;
  selectedChannelId: string | null;
}) {
  const t = useT();
  const [view, setView] = React.useState<ChatsViewMode>("all");
  const channels = React.useMemo(
    () => agentChatChannels(rows ?? [], agentPubkey),
    [agentPubkey, rows],
  );
  const directMessages = React.useMemo(
    () => channels.filter((channel) => channel.channelType === "dm"),
    [channels],
  );
  const groupChats = React.useMemo(
    () => channels.filter((channel) => isGroupChat(channel)),
    [channels],
  );
  const { dmChannelLabels, dmParticipantsByChannelId, dmPresenceByChannelId } =
    useDmSidebarMetadata({
      currentPubkey: agentPubkey,
      directMessages,
      groupChats,
    });
  const display = React.useMemo<ChatRowDisplay>(() => {
    const byId = new Map(channels.map((channel) => [channel.id, channel]));
    return {
      compact: true,
      tagOf: (channelId) => {
        const channel = byId.get(channelId);
        if (!channel) return null;
        if (channel.channelType === "dm") {
          return dmParticipantsByChannelId[channelId]?.[0]?.isAgent
            ? t("ownerView.tag.agent")
            : t("ownerView.tag.person");
        }
        return t("ownerView.tag.people", { count: channel.memberCount });
      },
    };
  }, [channels, dmParticipantsByChannelId, t]);
  const serverChats = React.useMemo<ServerChats>(
    () => ({
      getReadAt: noReadAt,
      onOpenThread,
      openRootId,
      readStateVersion: 0,
      rows: rows ?? [],
    }),
    [onOpenThread, openRootId, rows],
  );

  return (
    <div
      className="flex min-h-0 w-[320px] shrink-0 flex-col border-r border-border/70 bg-muted/40"
      data-testid="agent-conversations-list"
    >
      <div className="flex items-center gap-1.5 px-4 pb-2 pt-3 text-sm">
        <Eye aria-hidden="true" className="h-4 w-4 text-muted-foreground" />
        <span className="font-semibold">
          {t("ownerView.list.title", { name: agentName })}
        </span>
        <span className="text-muted-foreground">
          · {t("ownerView.list.subtitle")}
        </span>
      </div>
      <ChatsViewToggle onChange={setView} value={view} />
      {/* The open chat stands out on the grey column. */}
      <div className="kuzz-sidebar-scrollbar min-h-0 flex-1 overflow-y-auto px-[3px] pb-3 [&_[data-conversation-row][aria-current=true]>button>span]:!bg-muted">
        {rows === undefined ? null : rows.length === 0 ? (
          <p
            className="px-4 py-3 text-sm text-muted-foreground"
            data-testid="agent-conversations-empty"
          >
            {t("ownerView.list.empty", { name: agentName })}
          </p>
        ) : (
          <ChatRowDisplayContext.Provider value={display}>
            <ChatList
              channelLabels={dmChannelLabels}
              dmParticipantsByChannelId={dmParticipantsByChannelId}
              hideHeader
              isActiveChannel
              items={channels}
              onSelectChannel={onOpenChat}
              presenceByChannelId={dmPresenceByChannelId}
              selectedChannelId={selectedChannelId}
              serverChats={serverChats}
              unreadChannelCounts={NO_COUNTS}
              unreadChannelIds={NO_IDS}
              view={view}
            />
          </ChatRowDisplayContext.Provider>
        )}
      </div>
    </div>
  );
}
