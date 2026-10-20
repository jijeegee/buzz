import { FileText } from "lucide-react";

import { GroupLetterAvatar } from "@/features/home/ui/ConversationLetterAvatar";
import { useIsProjectHomeChannel } from "@/features/projects/lib/projectHomeChannel";
import { ProjectChannelIcon } from "@/features/projects/ui/ProjectChannelIcon";
import type { Channel } from "@/shared/api/types";
import { cn } from "@/shared/lib/cn";

/**
 * Picture for a group: its letter avatar (lock badge when private), the
 * forum icon for forums, or the project mark on project homes. `className`
 * styles the icon variants; `avatarClassName` positions the letter avatar.
 */
export function ChannelGlyph({
  avatarClassName,
  avatarSize = "xs",
  channel,
  className,
}: {
  avatarClassName?: string;
  avatarSize?: "xs" | "sm" | "md" | "xl";
  channel: Pick<Channel, "channelType" | "id" | "visibility"> & {
    name?: string;
    picture?: string | null;
  };
  className?: string;
}) {
  const projectHome = useIsProjectHomeChannel(channel.id);
  const iconClass = cn("size-4 shrink-0", className);

  if (projectHome) {
    return <ProjectChannelIcon className={iconClass} />;
  }
  if (channel.channelType === "forum") {
    return <FileText className={iconClass} />;
  }
  return (
    <GroupLetterAvatar
      channelId={channel.id}
      className={avatarClassName}
      isPrivate={channel.visibility === "private"}
      label={channel.name}
      picture={channel.picture}
      size={avatarSize}
    />
  );
}
