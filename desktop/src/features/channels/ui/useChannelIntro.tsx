import * as React from "react";
import {
  Bot,
  FolderPlus,
  Plus,
  Sparkles,
  UserPlus,
  UsersRound,
} from "lucide-react";

import {
  getChannelIntroDescription,
  getChannelIntroKind,
} from "@/features/channels/ui/ChannelPane.helpers";
import {
  isWelcomeChannel,
  isWelcomeExperienceChannel,
} from "@/features/onboarding/welcome";
import { useIsProjectHomeChannel } from "@/features/projects/lib/projectHomeChannel";
import { ProjectChannelIcon } from "@/features/projects/ui/ProjectChannelIcon";
import type { Channel } from "@/shared/api/types";
import { isGroupChat } from "@/features/channels/lib/groupChat";
import { ChannelChatAvatar } from "@/features/channels/ui/ChannelChatAvatar";
import { useChatTitle } from "@/features/channels/useChatTitle";

type ChannelIntroAction = {
  description?: string;
  icon: React.ReactNode;
  label: string;
  onClick: () => void;
  testId?: string;
};

/**
 * Builds the empty-channel intro block (heading, description, action cards)
 * for the channel timeline. The Welcome channel gets its onboarding trio
 * (browse / create channel / create agent); other channels get contextual
 * member actions.
 */
export function useChannelIntro({
  activeChannel,
  onAddAgent,
  onAddFiles,
  onBrowseChannels,
  onCreateChannel,
  onOpenMembers,
  onWelcomeAddAgent,
}: {
  activeChannel: Channel | null;
  onAddAgent?: (options?: { beforeSend?: () => void }) => void;
  onAddFiles?: () => void;
  onBrowseChannels?: () => void;
  onCreateChannel?: () => void;
  onOpenMembers?: () => void;
  onWelcomeAddAgent?: () => void;
}) {
  const projectHome = useIsProjectHomeChannel(activeChannel?.id);
  const chatTitle = useChatTitle(activeChannel, activeChannel?.name ?? "");

  return React.useMemo(() => {
    if (!activeChannel || activeChannel.channelType === "dm") {
      return null;
    }

    if (isGroupChat(activeChannel)) {
      const actions: ChannelIntroAction[] = [];
      if (activeChannel.isMember && onOpenMembers) {
        actions.push({
          icon: <UserPlus aria-hidden className="h-5 w-5" />,
          label: "Add people",
          onClick: onOpenMembers,
          testId: "channel-intro-action-add-people",
        });
      }
      return {
        actions,
        avatar: (
          <ChannelChatAvatar
            channel={activeChannel}
            className="size-[60px]"
            size="xl"
          />
        ),
        channelId: activeChannel.id,
        channelKindLabel: "chat",
        channelName: chatTitle,
        description: null,
      };
    }

    const actions: ChannelIntroAction[] = [];
    if (isWelcomeExperienceChannel(activeChannel)) {
      if (onBrowseChannels) {
        actions.push({
          icon: <UsersRound aria-hidden className="h-6 w-6" />,
          label: "Find groups",
          onClick: onBrowseChannels,
          testId: "welcome-intro-action-browse-channels",
        });
      }

      if (onCreateChannel) {
        actions.push({
          icon: <Plus aria-hidden className="h-6 w-6" />,
          label: "New group",
          onClick: onCreateChannel,
          testId: "welcome-intro-action-create-channel",
        });
      }

      if (onWelcomeAddAgent) {
        actions.push({
          icon: <Bot aria-hidden className="h-6 w-6" />,
          label: "Create an agent",
          onClick: onWelcomeAddAgent,
          testId: "welcome-intro-action-create-agent",
        });
      }

      return {
        actions,
        channelKindLabel: isWelcomeChannel(activeChannel)
          ? "private welcome group"
          : getChannelIntroKind(activeChannel, projectHome),
        channelName: activeChannel.name,
        description: isWelcomeChannel(activeChannel)
          ? null
          : getChannelIntroDescription(activeChannel),
        icon: <Sparkles aria-hidden className="h-7 w-7" />,
      };
    }

    if (!activeChannel.archivedAt && activeChannel.isMember) {
      if (onAddFiles) {
        actions.push({
          description: "Add a repo.",
          icon: <FolderPlus aria-hidden className="h-5 w-5" />,
          label: "Add files",
          onClick: onAddFiles,
          testId: "channel-intro-action-add-files",
        });
      }

      if (onAddAgent) {
        actions.push({
          description: "Add an agent here.",
          icon: <Bot aria-hidden className="h-5 w-5" />,
          label: "Add agent",
          onClick: onAddAgent,
          testId: "channel-intro-action-create-agent",
        });
      }

      if (onOpenMembers) {
        actions.push({
          description: "Invite members.",
          icon: <UserPlus aria-hidden className="h-5 w-5" />,
          label: "Add people",
          onClick: onOpenMembers,
          testId: "channel-intro-action-add-people",
        });
      }
    }

    return {
      actions,
      channelId: activeChannel.id,
      channelKindLabel: getChannelIntroKind(activeChannel, projectHome),
      channelName: activeChannel.name,
      description: getChannelIntroDescription(activeChannel),
      hideBeginning: projectHome,
      isPrivate: activeChannel.visibility === "private",
      picture: activeChannel.picture ?? null,
      icon: projectHome ? (
        <ProjectChannelIcon className="h-7 w-7" />
      ) : undefined,
    };
  }, [
    activeChannel,
    chatTitle,
    onAddAgent,
    onAddFiles,
    onBrowseChannels,
    onCreateChannel,
    onOpenMembers,
    onWelcomeAddAgent,
    projectHome,
  ]);
}
