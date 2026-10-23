import { getChannelDetail } from "@/features/channels/lib/channelDescription";
import { isEphemeralChannel } from "@/features/channels/lib/ephemeralChannel";
import type { TimelineMessage } from "@/features/messages/types";
import type { ChannelIntroKind } from "@/features/messages/ui/ChannelIntroBlock";
import type { TypingIndicatorEntry } from "@/features/messages/useChannelTyping";
import type { Channel } from "@/shared/api/types";
import { KIND_SYSTEM_MESSAGE } from "@/shared/constants/kinds";
import { t } from "@/shared/i18n";

export function shouldUseFocusIdleDrawer({
  channelManagementOpen,
  hasAgentSession,
  hasIdleAuxiliaryPanel,
  hasIdlePanelCloseHandler,
  hasProfilePanel,
  hasThreadSurface,
  overrideThread = false,
  useSplitAuxiliaryPane,
}: {
  channelManagementOpen: boolean;
  hasAgentSession: boolean;
  hasIdleAuxiliaryPanel: boolean;
  hasIdlePanelCloseHandler: boolean;
  hasProfilePanel: boolean;
  hasThreadSurface: boolean;
  overrideThread?: boolean;
  useSplitAuxiliaryPane: boolean;
}): boolean {
  return (
    (useSplitAuxiliaryPane || overrideThread) &&
    !channelManagementOpen &&
    !hasAgentSession &&
    !hasProfilePanel &&
    (!hasThreadSurface || overrideThread) &&
    hasIdleAuxiliaryPanel &&
    hasIdlePanelCloseHandler
  );
}

/** Which intro sentence a group shows ("This is the beginning of the …"). */
export function getChannelIntroKindId(
  channel: Channel,
  projectHome = false,
): Exclude<ChannelIntroKind, "chat" | "welcome"> {
  if (projectHome) {
    return "project";
  }

  const isPrivate = channel.visibility === "private";
  const isEphemeral = isEphemeralChannel(channel);

  if (isPrivate && isEphemeral) {
    return "privateEphemeral";
  }
  if (isPrivate) {
    return "private";
  }
  if (isEphemeral) {
    return "ephemeral";
  }
  return "regular";
}

const CHANNEL_INTRO_KIND_LABEL_KEYS = {
  project: "channels.intro.kindProject",
  privateEphemeral: "channels.intro.kindPrivateEphemeral",
  private: "channels.intro.kindPrivate",
  ephemeral: "channels.intro.kindEphemeral",
  regular: "channels.intro.kindRegular",
} as const;

/** Kind name on its own, e.g. "private group". */
export function getChannelIntroKind(
  channel: Channel,
  projectHome = false,
): string {
  return t(
    CHANNEL_INTRO_KIND_LABEL_KEYS[getChannelIntroKindId(channel, projectHome)],
  );
}

export function getChannelIntroDescription(channel: Channel): string | null {
  return getChannelDetail(channel);
}

/** Whether a caller-owned auxiliary sheet should render ahead of a thread. */
export function shouldPrioritizeIdleAuxiliary(
  overrideThread: boolean,
  hasIdleAuxiliary: boolean,
) {
  return overrideThread && hasIdleAuxiliary;
}

export function isWelcomeSetupSystemMessage(message: TimelineMessage) {
  if (message.kind !== KIND_SYSTEM_MESSAGE) {
    return false;
  }

  try {
    const payload = JSON.parse(message.body) as { type?: string };
    return (
      payload.type === "channel_created" || payload.type === "member_joined"
    );
  } catch {
    return false;
  }
}

export function isChannelCreatedSystemMessage(message: TimelineMessage) {
  if (message.kind !== KIND_SYSTEM_MESSAGE) {
    return false;
  }

  try {
    return (
      (JSON.parse(message.body) as { type?: string }).type === "channel_created"
    );
  } catch {
    return false;
  }
}

export function mentionsKnownAgent(
  mentionPubkeys: string[],
  knownAgentPubkeys: ReadonlySet<string>,
) {
  return mentionPubkeys.some((pubkey) =>
    knownAgentPubkeys.has(pubkey.toLowerCase()),
  );
}

export function selectThreadComposerBotTypingPubkeys(
  entries: TypingIndicatorEntry[],
  threadHeadId: string | null,
) {
  if (!threadHeadId) return [];
  return entries
    .filter((entry) => entry.threadHeadId === threadHeadId)
    .map((entry) => entry.pubkey)
    .filter(
      (pubkey, index, all) =>
        all.findIndex(
          (candidate) => candidate.toLowerCase() === pubkey.toLowerCase(),
        ) === index,
    );
}
