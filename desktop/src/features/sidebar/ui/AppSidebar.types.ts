import type { AddCommunityPrefillRequest } from "@/features/communities/addCommunityPrefill";
import type { Community } from "@/features/communities/types";
import type { useSidebarRelayConnectionCard } from "@/features/sidebar/ui/useSidebarRelayConnectionCard";
import type { ServerChats } from "@/features/sidebar/ui/ChatList";
import type {
  Channel,
  ChannelVisibility,
  Profile,
  SearchHit,
} from "@/shared/api/types";

export type CollapsibleSidebarGroup =
  | "starred"
  | "channels"
  | "forums"
  | "directMessages";

export type CreateChannelKind = "stream" | "forum";

export type AppSidebarProps = {
  addCommunityPrefill?: AddCommunityPrefillRequest | null;
  activeCommunity: Community | null;
  channels: Channel[];
  currentPubkey?: string;
  fallbackDisplayName?: string;
  isAddCommunityOpen?: boolean;
  isLoading: boolean;
  isCreatingChannel: boolean;
  isCreatingForum: boolean;
  profile?: Profile;
  relayConnectionCard: ReturnType<typeof useSidebarRelayConnectionCard>;
  errorMessage?: string;
  selectedChannelId: string | null;
  selectedView:
    | "home"
    | "channel"
    | "messages"
    | "agents"
    | "workflows"
    | "pulse"
    | "projects"
    | "people";
  unreadChannelCounts: ReadonlyMap<string, number>;
  unreadChannelIds: ReadonlySet<string>;
  highPriorityUnreadChannelIds: ReadonlySet<string>;
  previewActivityChannelIds: ReadonlySet<string>;
  communities: Community[];
  onAddCommunity: (community: Community) => void;
  onAddCommunityOpenChange?: (open: boolean) => void;
  onCreateChannel: (input: {
    name: string;
    description?: string;
    visibility: ChannelVisibility;
    ttlSeconds?: number;
    templateId?: string;
    addDefaultAi?: boolean;
  }) => Promise<void>;
  onCreateForum: (input: {
    name: string;
    description?: string;
    visibility: ChannelVisibility;
    ttlSeconds?: number;
    templateId?: string;
    addDefaultAi?: boolean;
  }) => Promise<void>;
  onHideDm: (channelId: string) => void;
  onMarkChannelUnread: (channelId: string) => void;
  onMarkChannelRead: (
    channelId: string,
    lastMessageAt: string | null | undefined,
  ) => void;
  onMarkAllChannelsRead: () => void;
  onBrowseChannels?: (onCreated?: (channelId: string) => void) => void;
  onOpenDm: (input: { pubkeys: string[] }) => Promise<void>;
  onCreateAgent: () => void;
  onSelectHome: () => void;
  onSelectChannel: (channelId: string) => void;
  onOpenSearchResult: (hit: SearchHit, query: string) => void;
  /** Full channel set for global search, including channels outside the joined sidebar list. */
  searchChannels: Channel[];
  searchFocusRequests: readonly [global: number, channel: number];
  onNewMessage: () => void;
  onBackgroundClick?: () => void;
  isCreateChannelOpen?: boolean;
  isHuddleCompanionOpen?: boolean;
  onHuddleEnded?: (ephemeralChannelId: string | null) => void;
  onCreateChannelOpenChange?: (open: boolean) => void;
  mutedChannelIds?: ReadonlySet<string>;
  onMuteChannel?: (channelId: string) => void;
  onUnmuteChannel?: (channelId: string) => void;
  starredChannelIds?: ReadonlySet<string>;
  onStarChannel?: (channelId: string) => void;
  onUnstarChannel?: (channelId: string) => void;
  /** The relay's chats list; undefined keeps today's device-local list. */
  serverChats?: ServerChats;
};
