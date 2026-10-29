import * as React from "react";
import { AnimatePresence } from "motion/react";
import { useAppNavigation } from "@/app/navigation/useAppNavigation";
import { useMediaUpload } from "@/features/messages/lib/useMediaUpload";
import { ComposerDockBackdrop } from "@/features/messages/ui/ComposerDockBackdrop";
import { ComposerUploadProgressOverlay } from "@/features/messages/ui/ComposerUploadProgressOverlay";
import { MessageComposer } from "@/features/messages/ui/MessageComposer";
import { ComposerTimeoutBanner } from "@/features/moderation/ui/ComposerTimeoutBanner";
import { BlockedDmComposerBanner } from "@/features/channels/ui/BlockedDmComposerBanner";
import { useBlockedPeople } from "@/features/people/BlockedPeopleContext";
import { blockedDmPeer } from "@/features/people/lib/blockedDm";
import { useTimeoutActive } from "@/features/moderation/lib/timeoutStore";
import { isModerationDm } from "@/features/moderation/lib/moderationDm";
import { useRelaySelfQuery } from "@/features/moderation/hooks";
import { DropZoneOverlay } from "@/features/messages/ui/ComposerAttachments";
import {
  MessageThreadPanel,
  THREAD_LAYER_CLASS,
} from "@/features/messages/ui/MessageThreadPanel";
import { MessageQuoteScope } from "@/features/messages/ui/messageQuoteScope";
import { useComposerQuotes } from "@/features/channels/ui/useComposerQuotes";
import type { SendPreview } from "@/features/messages/hooks";
import { MessageThreadPanelSkeleton } from "@/features/messages/ui/MessageThreadPanelSkeleton";
import { ThreadRepliesErrorCard } from "@/features/messages/ui/MessageThreadReplyState";
import {
  MessageTimeline,
  type MessageTimelineHandle,
} from "@/features/messages/ui/MessageTimeline";
import { buildDirectMessageIntro } from "@/features/channels/lib/dmParticipantDisplay";
import {
  getDmHuddleMemberPubkeys,
  hasOtherDmParticipant,
} from "@/features/channels/lib/dmHuddleMembers";
import { buildVideoReviewPresentationByMessageId } from "@/features/messages/lib/videoReviewContext";
import { useComposerHeightPadding } from "@/features/messages/ui/useComposerHeightPadding";
import { UserProfilePanel } from "@/features/profile/ui/UserProfilePanel";
import { AgentSessionThreadPanel } from "@/features/channels/ui/AgentSessionThreadPanel";
import { ChannelManagementAuxiliaryPanel } from "@/features/channels/ui/ChannelManagementAuxiliaryPanel";
import { IdleAuxiliaryPanel } from "@/features/channels/ui/IdleAuxiliaryPanel";
import { RightAuxiliaryPane } from "@/features/channels/ui/RightAuxiliaryPane";
import {
  ThreadPanelSurface,
  useThreadPanelSurface,
} from "@/features/channels/ui/ThreadPanelSurface";
import { ThreadViewModeToggle } from "@/features/channels/ui/ThreadViewModeToggle";
import { FocusThreadDrawer } from "@/features/channels/ui/FocusThreadDrawer";
import { THREAD_SURFACE_KEY } from "@/features/channels/lib/threadFocusLayout";
import { getThreadPanelLayout } from "@/features/channels/lib/threadPanelLayout";
import { useLandAtLatest } from "@/features/channels/lib/landAtLatest";
import { useThreadViewMode } from "@/features/channels/lib/threadViewModePreference";
import { useThreadViewModeSwitch } from "@/features/channels/ui/useThreadViewModeSwitch";
import { useFocusDrawerPresence } from "@/features/channels/ui/useFocusDrawerPresence";
import { useChannelWorkingAgentPubkeys } from "@/features/agents/agentWorkingSignal";
import { useCardMintJobs } from "@/features/agents/cardMintStore";
import { BotActivityComposerAction } from "@/features/channels/ui/BotActivityBar";
import { ChannelComposerActivityAccessory } from "@/features/channels/ui/ChannelComposerActivityAccessory";
import {
  containsWelcomePersonaMention,
  WelcomeComposerGuidanceLayer,
} from "@/features/channels/ui/WelcomeComposerBanner";
import { useWelcomeComposerBanner } from "@/features/channels/ui/useWelcomeComposerBanner";
import {
  mentionsKnownAgent,
  selectThreadComposerBotTypingPubkeys,
  shouldPrioritizeIdleAuxiliary,
  shouldUseFocusIdleDrawer,
} from "@/features/channels/ui/ChannelPane.helpers";
import { HuddleStartingView, HuddleTranscriptIntro } from "@/features/huddle";
import { useSearchHighlightProps } from "@/features/channels/ui/useSearchHighlightProps";
import { useChannelIntro } from "@/features/channels/ui/useChannelIntro";
import type { ChannelPaneProps } from "@/features/channels/ui/ChannelPane.types";
import * as agentSessionSelection from "@/features/channels/ui/agentSessionSelection";
import {
  peopleOutsideDm,
  usePrepareDmSendChannel,
} from "@/features/channels/ui/usePrepareDmSendChannel";
import { useStartChatConfirm } from "@/features/channels/ui/StartChatConfirm";
import { useChatTitle } from "@/features/channels/useChatTitle";
import { useChannelPaneMessages } from "@/features/channels/ui/useChannelPaneMessages";
import { useRoutedMessageEdit } from "@/features/channels/ui/useRoutedMessageEdit";
import { Button } from "@/shared/ui/button";
import { useRenderScopedReactionHydration } from "@/features/messages/lib/useRenderScopedReactionHydration";
import { isWelcomeExperienceChannel as isWelcomeExperience } from "@/features/onboarding/welcome";
import { useIsThreadPanelOverlay } from "@/shared/hooks/use-mobile";
import { channelChrome } from "@/shared/layout/chromeLayout";
import { cn } from "@/shared/lib/cn";
import { useThreadOpenTarget } from "./useThreadOpenTarget";
import { useT } from "@/shared/i18n";
const HUDDLE_TRANSCRIPT_ROOT_STYLE = {
  "--kuzz-channel-content-top-padding": "0rem",
  "--channel-top-chrome-height": "0.25rem",
} as React.CSSProperties;
export const ChannelPane = React.memo(function ChannelPane({
  activeChannel,
  agentPubkeys,
  agentPubkeysPending = false,
  agentSessionAgents,
  activityAgents = agentSessionAgents,
  autoSendDraftKey = null,
  onAutoSendComplete = null,
  botTypingEntries,
  channelManagementOpen = false,
  channelManagementInitialView = null,
  currentPubkey,
  readingPositionScope = null,
  editTarget = null,
  fetchOlder,
  header,
  idleAuxiliaryPanel = null,
  idleAuxiliaryHeaderActions,
  idleAuxiliaryOverridesThread = false,
  idleAuxiliaryTitle = "",
  hasOlderMessages,
  historyExhausted,
  isFetchingOlder,
  isHuddleTranscript = false,
  followThreadById,
  isFollowingThread,
  isFollowingThreadById,
  isMessageUnreadById,
  isJoining = false,
  isSinglePanelView = false,
  isSending,
  isTimelineError = false,
  isTimelineLoading,
  onRetryTimeline,
  entranceMessageId = null,
  onEntranceMessageComplete,
  welcomeKickoffStage = null,
  welcomeKickoffSettingUp = false,
  messages,
  threadSummaries,
  huddleThreadRepliesError = false,
  onRetryHuddleThreadReplies,
  firstUnreadMessageId = null,
  initialAnchorMessageId = null,
  initialAnchorTopPx = null,
  onReadingPositionChange,
  unreadCount = 0,
  canResetThreadPanelWidth,
  onCancelEdit,
  onCancelThreadReply,
  onBackFromAgentSession,
  onCloseAgentSession,
  onCloseChannelManagement,
  onOpenGoalLink,
  onChannelManagementDeleted,
  onCloseIdleAuxiliaryPanel,
  onCloseProfilePanel,
  onAddAgent,
  onAddFiles,
  onBrowseChannels,
  onCreateChannel,
  onCloseThread,
  onDelete,
  onEdit,
  onEditSave,
  onFollowThread,
  onMarkUnread,
  onMarkRead,
  onExpandThreadReplies,
  onJoinChannel,
  onOpenAgentSession,
  onOpenDm,
  onOpenMembers,
  onOpenProfilePanel,
  onOpenThread,
  onPreviewSend,
  onResetThreadPanelWidth,
  onSelectThreadReplyTarget,
  onSendMessage,
  onSendToChannel,
  onSendVideoReviewComment,
  onSendThreadReply,
  onThreadScrollTargetResolved,
  onThreadPanelResizeStart,
  onTargetReached,
  onToggleReaction,
  onUnfollowThread,
  readOnlyFooter,
  unfollowThreadById,
  personaLookup,
  profiles,
  ownerProfiles,
  openThreadHeadId,
  shouldShowThreadSkeleton,
  openAgentSessionChannelId,
  openAgentSessionPubkey,
  onProfilePanelViewChange,
  onProfilePanelTabChange,
  profilePanelPubkey,
  profilePanelTab,
  profilePanelView,
  targetMessageId,
  targetSearchMessageId,
  targetSearchQuery,
  threadAllMessages,
  threadHeadMessage,
  threadMessages,
  threadMessagesPending = false,
  threadMessagesError = false,
  onRetryThreadReplies,
  threadHasOlderReplies = false,
  threadIsFetchingOlderReplies = false,
  onFetchOlderThreadReplies,
  onShowThreadInChannel,
  threadPanelWidthPx,
  threadScrollTargetId,
  threadTypingPubkeys,
  threadReplyTargetMessage,
  threadUnreadCounts,
  threadReplyUnreadCounts,
  threadFirstUnreadReplyId,
  threadOpenRead = "ready",
  typingPubkeys,
}: ChannelPaneProps) {
  const t = useT();
  const timelineScrollRef = React.useRef<HTMLDivElement>(null);
  const messageTimelineRef = React.useRef<MessageTimelineHandle>(null);
  const composerWrapperRef = React.useRef<HTMLDivElement>(null);
  const { goChannel } = useAppNavigation();
  const startChatConfirm = useStartChatConfirm();
  const activeChannelTitle = useChatTitle(
    activeChannel,
    activeChannel?.name ?? "",
  );
  const prepareDmSendChannel = usePrepareDmSendChannel(
    activeChannel,
    currentPubkey,
    startChatConfirm.confirm,
  );
  const dmSendStaysInChannel = React.useCallback(
    (pubkeys: readonly string[]) =>
      activeChannel?.channelType === "dm" &&
      peopleOutsideDm(activeChannel.participantPubkeys, pubkeys, currentPubkey)
        .length === 0,
    [activeChannel, currentPubkey],
  );
  const mainComposerMedia = useMediaUpload({ deferUploadsUntilSend: true });
  const searchHighlightProps = useSearchHighlightProps(
    targetSearchMessageId,
    targetSearchQuery,
  );
  const [isMainDeferredEditPending, setMainDeferredEditPending] =
    React.useState(false);
  const [acceptsMainAttachments, setAcceptsMainAttachments] =
    React.useState(true);
  const isNonMemberView =
    !readOnlyFooter &&
    activeChannel !== null &&
    !activeChannel.isMember &&
    activeChannel.visibility === "open" &&
    !activeChannel.archivedAt;
  const hasMainComposerOverlay = !isNonMemberView && !readOnlyFooter;
  const activeChannelId = activeChannel?.id ?? null;
  const activeChannelIdRef = React.useRef(activeChannelId);
  const channelPaneMountedRef = React.useRef(false);
  activeChannelIdRef.current = activeChannelId;
  React.useEffect(() => {
    channelPaneMountedRef.current = true;
    return () => {
      channelPaneMountedRef.current = false;
    };
  }, []);
  const handleAutoSubmitComplete = React.useCallback(() => {
    if (onAutoSendComplete) {
      onAutoSendComplete();
    } else if (activeChannelId) {
      void goChannel(activeChannelId, { replace: true });
    }
  }, [activeChannelId, goChannel, onAutoSendComplete]);
  const huddleMemberPubkeys = React.useMemo(
    () => getDmHuddleMemberPubkeys(activeChannel, agentPubkeys, currentPubkey),
    [activeChannel, agentPubkeys, currentPubkey],
  );
  const huddleMemberPubkeysPending =
    agentPubkeysPending && hasOtherDmParticipant(activeChannel, currentPubkey);
  const isActiveWelcomeChannel =
    activeChannel !== null && isWelcomeExperience(activeChannel);
  useComposerHeightPadding(
    timelineScrollRef,
    composerWrapperRef,
    `${activeChannelId}:${isSinglePanelView}:${hasMainComposerOverlay}`,
    "css-variable",
    () => messageTimelineRef.current?.settleAtBottom() ?? false,
  );
  // A channel row in the inbox lands on the channel's newest message.
  useLandAtLatest(
    activeChannelId,
    () => messageTimelineRef.current?.settleAtBottom() ?? false,
  );
  const {
    bannerState: welcomeComposerBannerState,
    completeBanner: completeWelcomeComposerBanner,
    dismissBanner: handleDismissWelcomeBanner,
  } = useWelcomeComposerBanner(
    activeChannelId,
    isActiveWelcomeChannel,
    currentPubkey ?? null,
  );
  const isEditInThread = editTarget?.isThreadReply === true;
  const mainEditTarget = editTarget && !isEditInThread ? editTarget : null;
  const threadEditTarget = editTarget && isEditInThread ? editTarget : null;
  const timeoutActive = useTimeoutActive();
  const relaySelfQuery = useRelaySelfQuery(activeChannel?.channelType === "dm");
  const isModerationDmChannel = isModerationDm(
    activeChannel ?? null,
    currentPubkey,
    relaySelfQuery.data,
  );
  // A 1:1 DM with someone I blocked: no typing or sending until I unblock.
  const { isBlocked } = useBlockedPeople();
  const blockedPeer = blockedDmPeer(activeChannel, currentPubkey, isBlocked);
  const isComposerDisabled =
    blockedPeer !== null ||
    !activeChannel?.isMember ||
    activeChannel.archivedAt !== null ||
    activeChannel.channelType === "forum" ||
    timeoutActive ||
    isModerationDmChannel ||
    isSending;
  const knownAgentPubkeys = React.useMemo(() => {
    const pubkeys = new Set<string>();
    for (const pubkey of agentPubkeys ?? []) {
      pubkeys.add(pubkey.toLowerCase());
    }
    for (const agent of agentSessionAgents) {
      pubkeys.add(agent.pubkey.toLowerCase());
    }
    for (const agent of activityAgents) {
      pubkeys.add(agent.pubkey.toLowerCase());
    }
    return pubkeys;
  }, [activityAgents, agentPubkeys, agentSessionAgents]);
  const handleSendMessage = React.useCallback(
    async (
      content: string,
      mentionPubkeys: string[],
      mediaTags?: string[][],
      channelId?: string | null,
      threadContext?: {
        parentEventId: string | null;
        threadHeadId: string | null;
      } | null,
      forceRest?: boolean,
      optimisticLocalKey?: string,
    ) => {
      const shouldCompleteWelcomeBanner =
        isActiveWelcomeChannel &&
        (containsWelcomePersonaMention(content) ||
          mentionsKnownAgent(mentionPubkeys, knownAgentPubkeys));
      messageTimelineRef.current?.scrollToBottomOnNextUpdate();
      const sentMessage = await onSendMessage(
        content,
        mentionPubkeys,
        mediaTags,
        channelId,
        threadContext,
        forceRest,
        optimisticLocalKey,
      );
      if (
        channelId &&
        channelId !== activeChannelId &&
        channelPaneMountedRef.current &&
        activeChannelIdRef.current === activeChannelId
      ) {
        await goChannel(channelId, { replace: true });
      }
      if (shouldCompleteWelcomeBanner) {
        completeWelcomeComposerBanner();
      }
      return sentMessage;
    },
    [
      activeChannelId,
      completeWelcomeComposerBanner,
      goChannel,
      isActiveWelcomeChannel,
      knownAgentPubkeys,
      onSendMessage,
    ],
  );
  const previewMainSend = React.useMemo<SendPreview | undefined>(
    () =>
      onPreviewSend &&
      ((input) => {
        messageTimelineRef.current?.scrollToBottomOnNextUpdate();
        return onPreviewSend(input);
      }),
    [onPreviewSend],
  );
  // Quote lands in the composer of the conversation the row belongs to: the
  // main timeline's composer or the open thread's composer.
  const composerQuotes = useComposerQuotes({
    activeChannelId,
    enabled:
      !isHuddleTranscript &&
      Boolean(activeChannel?.isMember) &&
      !activeChannel?.archivedAt &&
      activeChannel?.channelType !== "forum" &&
      !isModerationDmChannel &&
      blockedPeer === null,
    previewMain: previewMainSend,
    previewThread: onPreviewSend,
    sendMain: handleSendMessage,
    sendThread: onSendThreadReply,
    threadHeadId: threadHeadMessage?.id ?? null,
  });
  const canDropInMainColumn =
    hasMainComposerOverlay &&
    !isComposerDisabled &&
    !isMainDeferredEditPending &&
    acceptsMainAttachments &&
    !isSinglePanelView;
  const hasTypingActivity = typingPubkeys.length > 0;
  const composerWorkingBotPubkeys = useChannelWorkingAgentPubkeys(
    activeChannel?.id ?? null,
  );
  const hasComposerBotActivity = composerWorkingBotPubkeys.length > 0;
  const hasCardMintActivity = useCardMintJobs().length > 0;
  const hasComposerBottomActivity =
    hasComposerBotActivity || hasTypingActivity || hasCardMintActivity;
  const threadComposerBotTypingPubkeys = React.useMemo(
    () =>
      selectThreadComposerBotTypingPubkeys(botTypingEntries, openThreadHeadId),
    [botTypingEntries, openThreadHeadId],
  );
  const hasThreadComposerBotActivity =
    threadComposerBotTypingPubkeys.length > 0;
  const directMessageIntro = React.useMemo(
    () =>
      buildDirectMessageIntro({
        channel: activeChannel,
        currentPubkey,
        profiles,
      }),
    [activeChannel, currentPubkey, profiles],
  );
  const handleWelcomeAddAgent = React.useCallback(() => {
    onAddAgent?.({
      beforeSend: () =>
        messageTimelineRef.current?.scrollToBottomOnNextUpdate(),
    });
  }, [onAddAgent]);
  const standardChannelIntro = useChannelIntro({
    activeChannel,
    onAddAgent,
    onAddFiles,
    onBrowseChannels,
    onCreateChannel,
    onOpenMembers,
    onWelcomeAddAgent: onAddAgent ? handleWelcomeAddAgent : undefined,
  });
  const channelIntro = isHuddleTranscript ? null : standardChannelIntro;
  const { mainTimelineEntries, recentMentions, visibleMessages } =
    useChannelPaneMessages({
      activeChannel,
      isHuddleTranscript,
      messages,
      profiles,
      threadSummaries,
    });
  useRenderScopedReactionHydration({
    activeChannel,
    mainTimelineEntries,
    threadHeadMessage,
    threadMessages,
  });
  const activeVideoReviewCommentSender = activeChannel?.archivedAt
    ? undefined
    : onSendVideoReviewComment;
  const threadVideoReviewPresentation = React.useMemo(() => {
    const messagesById = new Map(
      messages.map((message) => [message.id, message]),
    );
    if (threadHeadMessage) {
      messagesById.set(threadHeadMessage.id, threadHeadMessage);
    }
    for (const message of threadAllMessages) {
      messagesById.set(message.id, message);
    }
    return buildVideoReviewPresentationByMessageId({
      channelId: activeChannel?.id ?? null,
      channelName: activeChannel?.name,
      channelType: activeChannel?.channelType ?? null,
      isSendingVideoReviewComment: isSending,
      messages: [...messagesById.values()],
      onSendVideoReviewComment: activeVideoReviewCommentSender,
      onToggleReaction,
      profiles,
    });
  }, [
    activeChannel,
    activeVideoReviewCommentSender,
    isSending,
    messages,
    onToggleReaction,
    profiles,
    threadAllMessages,
    threadHeadMessage,
  ]);
  const isOverlay = useIsThreadPanelOverlay();
  const useSplitAuxiliaryPane = !isSinglePanelView && !isOverlay;
  const threadViewMode = useThreadViewMode();
  const hasThreadSurface =
    Boolean(threadHeadMessage) || shouldShowThreadSkeleton;
  const useFocusThreadDrawer =
    threadViewMode === "focus" && useSplitAuxiliaryPane && hasThreadSurface;
  const selectedAgent = React.useMemo(
    () =>
      agentSessionSelection.resolveSelectedAgentSession({
        agentSessionAgents,
        openAgentSessionPubkey,
        profilePanelPubkey,
        profiles,
      }),
    [agentSessionAgents, openAgentSessionPubkey, profilePanelPubkey, profiles],
  );
  const hasIdleAuxiliary =
    Boolean(idleAuxiliaryPanel) && Boolean(onCloseIdleAuxiliaryPanel);
  const priorityIdleAuxiliary = shouldPrioritizeIdleAuxiliary(
    idleAuxiliaryOverridesThread,
    hasIdleAuxiliary,
  );
  const overlayIdleAuxiliaryOverThread =
    priorityIdleAuxiliary && hasThreadSurface && !isOverlay;
  const replaceThreadWithIdleAuxiliary =
    priorityIdleAuxiliary && hasThreadSurface && isOverlay;
  const useFocusIdleDrawer = shouldUseFocusIdleDrawer({
    channelManagementOpen,
    hasAgentSession: Boolean(activeChannel && selectedAgent),
    hasIdleAuxiliaryPanel: Boolean(idleAuxiliaryPanel),
    hasIdlePanelCloseHandler: Boolean(onCloseIdleAuxiliaryPanel),
    hasProfilePanel: Boolean(profilePanelPubkey),
    hasThreadSurface,
    overrideThread: overlayIdleAuxiliaryOverThread,
    useSplitAuxiliaryPane,
  });
  const showIdleAuxiliaryOverThread =
    overlayIdleAuxiliaryOverThread && useFocusIdleDrawer;
  const { channelIsCovered, markExitComplete } = useFocusDrawerPresence(
    useFocusThreadDrawer || useFocusIdleDrawer,
    priorityIdleAuxiliary
      ? (onCloseIdleAuxiliaryPanel ?? onCloseThread)
      : useFocusThreadDrawer
        ? onCloseThread
        : (onCloseIdleAuxiliaryPanel ?? onCloseThread),
  );
  const threadSurface = useThreadPanelSurface(
    showIdleAuxiliaryOverThread,
    markExitComplete,
  );
  const threadOpen = useThreadOpenTarget({
    channelId: activeChannel?.id ?? null,
    rootId: threadHeadMessage?.id ?? null,
    positionScope: readingPositionScope,
    opensElsewhere: Boolean(threadScrollTargetId),
    replies: threadMessages,
    isLoading: threadMessagesPending,
    firstUnreadReplyId: threadFirstUnreadReplyId ?? null,
    readState: threadOpenRead,
    hasOlderReplies: threadHasOlderReplies,
    isFetchingOlder: threadIsFetchingOlderReplies,
    fetchOlder: onFetchOlderThreadReplies,
  });
  const { changeThreadViewMode, layoutScrollTargetId, resolveScrollTarget } =
    useThreadViewModeSwitch({
      activeThreadHeadId: threadHeadMessage?.id ?? null,
      externalScrollTargetId: threadScrollTargetId,
      onExternalTargetResolved: onThreadScrollTargetResolved,
      onModeChange: markExitComplete,
    });
  const {
    handleEditLastOwnMainMessage,
    handleEditLastOwnThreadMessage,
    routeEdit: handleRoutedEdit,
  } = useRoutedMessageEdit({
    activeChannelId,
    channelIsCovered,
    currentPubkey,
    editTarget,
    isSinglePanelView,
    mainMessages: mainTimelineEntries.map((entry) => entry.message),
    onCloseThread,
    onEdit,
    threadHeadMessage,
    threadMessages: threadMessages.map((entry) => entry.message),
    useFocusThreadDrawer,
  });
  const hasSplitAuxiliaryPane =
    useSplitAuxiliaryPane &&
    (channelManagementOpen ||
      Boolean(threadHeadMessage) ||
      shouldShowThreadSkeleton ||
      Boolean(activeChannel && selectedAgent) ||
      Boolean(profilePanelPubkey));
  const wrapAux = (
    panel: React.ReactNode,
    testId: string,
    options: { className?: string; key?: string } = {},
  ) =>
    useSplitAuxiliaryPane ? (
      <RightAuxiliaryPane
        canResetWidth={canResetThreadPanelWidth}
        className={options.className}
        key={options.key ?? testId}
        onResetWidth={onResetThreadPanelWidth}
        onResizeStart={onThreadPanelResizeStart}
        testId={testId}
        widthPx={threadPanelWidthPx}
      >
        {panel}
      </RightAuxiliaryPane>
    ) : (
      <React.Fragment key={options.key ?? testId}>{panel}</React.Fragment>
    );
  const wrapThreadPanel = (panel: React.ReactNode) => (
    <ThreadPanelSurface
      channelName={activeChannel?.name ?? t("channels.pane.groupFallback")}
      covered={threadSurface.covered}
      hasActiveEdit={threadEditTarget !== null}
      isFocusDrawer={useFocusThreadDrawer}
      key={THREAD_SURFACE_KEY}
      onClose={onCloseThread}
      ref={threadSurface.ref}
    >
      {useFocusThreadDrawer
        ? panel
        : wrapAux(panel, "message-thread-panel", {
            className: THREAD_LAYER_CLASS,
          })}
    </ThreadPanelSurface>
  );
  const wrapIdlePanel = (panel: React.ReactNode) =>
    useFocusIdleDrawer && onCloseIdleAuxiliaryPanel ? (
      <FocusThreadDrawer
        channelName={activeChannel?.name ?? t("channels.pane.groupFallback")}
        key="idle-auxiliary-surface"
        label={idleAuxiliaryTitle || t("channels.pane.panelFallback")}
        onClose={onCloseIdleAuxiliaryPanel}
        restoreFocusTarget={threadSurface.restoreFocusTarget}
      >
        {panel}
      </FocusThreadDrawer>
    ) : (
      wrapAux(panel, "idle-auxiliary-panel")
    );
  const idleAuxiliarySurface =
    idleAuxiliaryPanel && onCloseIdleAuxiliaryPanel
      ? wrapIdlePanel(
          <IdleAuxiliaryPanel
            canResetWidth={canResetThreadPanelWidth}
            headerControls={idleAuxiliaryHeaderActions}
            isFocusDrawer={useFocusIdleDrawer}
            isSinglePanelView={isSinglePanelView}
            onClose={onCloseIdleAuxiliaryPanel}
            onResetWidth={onResetThreadPanelWidth}
            onResizeStart={onThreadPanelResizeStart}
            title={idleAuxiliaryTitle}
            useSplitAuxiliaryPane={useSplitAuxiliaryPane}
            widthPx={threadPanelWidthPx}
          >
            {idleAuxiliaryPanel}
          </IdleAuxiliaryPanel>,
        )
      : null;
  const threadHeaderLeading = useSplitAuxiliaryPane ? (
    <ThreadViewModeToggle onChange={changeThreadViewMode} />
  ) : undefined;
  const threadLayoutProps = getThreadPanelLayout({
    headerLeading: threadHeaderLeading,
    isFocusDrawer: useFocusThreadDrawer,
    isSinglePanelView,
    useSplitAuxiliaryPane,
  });
  const timelineReplyHandler =
    activeChannel?.archivedAt || isHuddleTranscript ? undefined : onOpenThread;
  return (
    <div
      className="relative flex min-h-0 min-w-0 flex-1 flex-row overflow-hidden"
      style={isHuddleTranscript ? HUDDLE_TRANSCRIPT_ROOT_STYLE : undefined}
    >
      {!isSinglePanelView && !isHuddleTranscript ? (
        <div
          aria-hidden="true"
          className={cn(
            "pointer-events-none absolute inset-x-0 top-0 z-30 bg-background/80 backdrop-blur-md supports-backdrop-filter:bg-background/70 dark:bg-background/70 dark:backdrop-blur-xl dark:supports-backdrop-filter:bg-background/55",
            channelChrome.headerHeight,
          )}
          data-testid="channel-shared-header-backdrop"
        />
      ) : null}
      {!isSinglePanelView ? (
        <section
          aria-label={t("channels.pane.messagesLabel")}
          className="relative flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden"
          inert={channelIsCovered ? true : undefined}
          data-testid="channel-drop-zone"
          onDragEnter={
            canDropInMainColumn ? mainComposerMedia.handleDragEnter : undefined
          }
          onDragLeave={
            canDropInMainColumn ? mainComposerMedia.handleDragLeave : undefined
          }
          onDragOver={
            canDropInMainColumn ? mainComposerMedia.handleDragOver : undefined
          }
          onDrop={
            canDropInMainColumn
              ? (event) => {
                  void mainComposerMedia.handleDrop(event);
                }
              : undefined
          }
        >
          {isHuddleTranscript ? null : header}
          {isHuddleTranscript && huddleThreadRepliesError ? (
            <div className="px-5 pt-3">
              <ThreadRepliesErrorCard onRetry={onRetryHuddleThreadReplies} />
            </div>
          ) : null}
          <MessageQuoteScope value={composerQuotes.mainScope}>
            <div className="relative isolate flex min-h-0 min-w-0 flex-1 flex-col">
              <MessageTimeline
                ref={messageTimelineRef}
                channelId={activeChannel?.id}
                channelIntro={channelIntro}
                directMessageIntro={directMessageIntro}
                scrollContainerRef={timelineScrollRef}
                currentPubkey={currentPubkey}
                fetchOlder={fetchOlder}
                followThreadById={followThreadById}
                hasComposerOverlay={hasMainComposerOverlay}
                hasOlderMessages={hasOlderMessages}
                historyExhausted={historyExhausted}
                hideDayDividers={isHuddleTranscript}
                alwaysShowMessageIdentity={isHuddleTranscript}
                hideAgentAccessBadges={isHuddleTranscript}
                pinnedIntro={
                  isHuddleTranscript ? <HuddleTranscriptIntro /> : undefined
                }
                huddleMemberPubkeys={huddleMemberPubkeys}
                huddleMemberPubkeysPending={huddleMemberPubkeysPending}
                isFetchingOlder={isFetchingOlder}
                isFollowingThreadById={isFollowingThreadById}
                isMessageUnreadById={isMessageUnreadById}
                personaLookup={personaLookup}
                profiles={profiles}
                ownerProfiles={ownerProfiles}
                unfollowThreadById={unfollowThreadById}
                emptyDescription={
                  activeChannel?.channelType === "forum"
                    ? t("channels.pane.forumEmptyBody")
                    : t("channels.pane.emptyBody")
                }
                emptyTitle={
                  activeChannel
                    ? activeChannel.channelType === "forum"
                      ? t("channels.pane.forumEmptyTitle")
                      : t("channels.pane.emptyTitle")
                    : t("channels.canvas.noGroupSelected")
                }
                isError={isTimelineError}
                isLoading={isHuddleTranscript ? false : isTimelineLoading}
                onRetry={onRetryTimeline}
                entranceMessageId={entranceMessageId}
                onEntranceMessageComplete={onEntranceMessageComplete}
                mainEntries={mainTimelineEntries}
                threadSummaries={threadSummaries}
                messages={visibleMessages}
                firstUnreadMessageId={firstUnreadMessageId}
                initialAnchorMessageId={initialAnchorMessageId}
                initialAnchorTopPx={initialAnchorTopPx}
                onReadingPositionChange={onReadingPositionChange}
                unreadCount={unreadCount}
                onDelete={onDelete}
                onEdit={handleRoutedEdit}
                onMarkUnread={onMarkUnread}
                onMarkRead={onMarkRead}
                onReply={timelineReplyHandler}
                onOpenThread={isHuddleTranscript ? undefined : onOpenThread}
                channelName={activeChannel?.name}
                channelType={activeChannel?.channelType ?? null}
                isSendingVideoReviewComment={isSending}
                onSendVideoReviewComment={
                  activeChannel?.archivedAt
                    ? undefined
                    : onSendVideoReviewComment
                }
                onTargetReached={onTargetReached}
                onToggleReaction={onToggleReaction}
                {...searchHighlightProps.timeline}
                targetMessageId={targetMessageId}
                splitThreadPanelOpen={
                  useSplitAuxiliaryPane &&
                  !useFocusThreadDrawer &&
                  Boolean(openThreadHeadId)
                }
                threadUnreadCounts={threadUnreadCounts}
              />
              {readOnlyFooter ? (
                <div
                  className="border-t border-border/80 bg-muted/60"
                  data-testid="owner-view-footer"
                >
                  {readOnlyFooter}
                </div>
              ) : isNonMemberView ? (
                // Public group preview: read-only timeline, and one
                // full-width Join group in place of the composer.
                <div
                  data-testid="join-banner"
                  className="border-t border-border/80 bg-card/50 px-5 py-3"
                >
                  <Button
                    className="w-full"
                    data-testid="join-group-button"
                    disabled={isJoining}
                    onClick={() => {
                      void onJoinChannel?.();
                    }}
                    variant="default"
                  >
                    {isJoining
                      ? t("channels.browser.joining")
                      : t("channels.settings.join")}
                  </Button>
                </div>
              ) : (
                <div
                  className="pointer-events-none absolute inset-x-0 bottom-0 z-40 isolate before:absolute before:inset-x-0 before:bottom-0 before:-z-10 before:h-24 before:bg-gradient-to-b before:from-transparent before:to-background before:content-[''] after:absolute after:inset-x-0 after:bottom-0 after:-z-10 after:h-12 after:bg-background after:content-['']"
                  data-testid="channel-composer-overlay"
                  ref={composerWrapperRef}
                >
                  <ComposerUploadProgressOverlay />
                  <div
                    className={cn(
                      "composer-dock composer-overlay-corner-masks relative pointer-events-auto",
                      hasComposerBottomActivity &&
                        "composer-dock--with-activity",
                    )}
                  >
                    {isActiveWelcomeChannel && !timeoutActive ? (
                      <WelcomeComposerGuidanceLayer
                        onDismiss={handleDismissWelcomeBanner}
                        settingUp={welcomeKickoffSettingUp}
                        state={welcomeComposerBannerState}
                      >
                        {welcomeKickoffStage}
                      </WelcomeComposerGuidanceLayer>
                    ) : null}
                    {timeoutActive ? <ComposerTimeoutBanner /> : null}
                    {blockedPeer && activeChannel && !timeoutActive ? (
                      <BlockedDmComposerBanner
                        channel={activeChannel}
                        currentPubkey={currentPubkey}
                        peer={blockedPeer}
                        profiles={profiles}
                      />
                    ) : null}
                    <ComposerDockBackdrop gutterClassName="inset-x-5" />
                    <MessageComposer
                      audienceContext={
                        activeChannel && activeChannel.channelType !== "dm"
                          ? { type: "channel" }
                          : null
                      }
                      channelId={activeChannel?.id ?? null}
                      channelName={
                        activeChannel?.name ?? t("channels.pane.groupFallback")
                      }
                      channelType={activeChannel?.channelType ?? null}
                      containerClassName="px-5 pb-0"
                      layoutMode="dock"
                      disabled={isComposerDisabled}
                      editTarget={mainEditTarget}
                      autoSubmitDraftKey={autoSendDraftKey}
                      onAutoSubmitComplete={handleAutoSubmitComplete}
                      isSending={isSending}
                      mediaController={mainComposerMedia}
                      onAttachmentAcceptanceChange={setAcceptsMainAttachments}
                      onDeferredEditPendingChange={setMainDeferredEditPending}
                      onCancelEdit={onCancelEdit}
                      onEditLastOwnMessage={handleEditLastOwnMainMessage}
                      onEditSave={onEditSave}
                      onPrepareSendChannel={
                        activeChannel?.channelType === "dm"
                          ? prepareDmSendChannel
                          : undefined
                      }
                      sendStaysInChannel={dmSendStaysInChannel}
                      onPreviewSend={composerQuotes.previewMain}
                      onSend={composerQuotes.sendMain}
                      {...{ profiles, recentMentionPubkeys: recentMentions }}
                      showBackgroundUploadProgress={false}
                      placeholder={
                        timeoutActive
                          ? t("channels.composer.timedOut")
                          : blockedPeer
                            ? t("channels.composer.blocked")
                            : isModerationDmChannel
                              ? t("channels.composer.readOnly")
                              : activeChannel?.archivedAt
                                ? t("channels.composer.archived")
                                : activeChannel?.channelType === "forum"
                                  ? t("channels.composer.forum")
                                  : activeChannel
                                    ? activeChannel.channelType === "dm" &&
                                      directMessageIntro
                                      ? t("channels.composer.message", {
                                          name: directMessageIntro.displayName,
                                        })
                                      : t("channels.composer.message", {
                                          name: activeChannelTitle,
                                        })
                                    : t("channels.composer.selectGroup")
                      }
                      showTopBorder={false}
                    />
                    <ChannelComposerActivityAccessory
                      agents={activityAgents}
                      channel={activeChannel}
                      currentPubkey={currentPubkey}
                      onOpenAgentSession={onOpenAgentSession}
                      openAgentSessionPubkey={openAgentSessionPubkey}
                      profiles={profiles}
                      typingPubkeys={typingPubkeys}
                      visible={hasComposerBottomActivity}
                      workingBotPubkeys={composerWorkingBotPubkeys}
                    />
                  </div>
                </div>
              )}
              {canDropInMainColumn && mainComposerMedia.isDragOver ? (
                <DropZoneOverlay className="z-50 rounded-2xl bg-primary/20 backdrop-blur-sm" />
              ) : null}
            </div>
          </MessageQuoteScope>
        </section>
      ) : null}
      {startChatConfirm.dialog}
      {/* Serialize replacements so focus drawers keep one travel direction. */}
      <AnimatePresence mode="wait" onExitComplete={markExitComplete}>
        {channelManagementOpen && activeChannel ? (
          <ChannelManagementAuxiliaryPanel
            activeChannel={activeChannel}
            canResetThreadPanelWidth={canResetThreadPanelWidth}
            currentPubkey={currentPubkey}
            isSinglePanelView={isSinglePanelView}
            key="channel-management-panel"
            onChannelManagementDeleted={onChannelManagementDeleted}
            onCloseChannelManagement={onCloseChannelManagement}
            initialView={channelManagementInitialView}
            onOpenGoalLink={onOpenGoalLink}
            onOpenMembers={onOpenMembers}
            onResetThreadPanelWidth={onResetThreadPanelWidth}
            onThreadPanelResizeStart={onThreadPanelResizeStart}
            threadPanelWidthPx={threadPanelWidthPx}
            useSplitAuxiliaryPane={useSplitAuxiliaryPane}
            transparentChrome={hasSplitAuxiliaryPane}
          />
        ) : replaceThreadWithIdleAuxiliary && idleAuxiliarySurface ? (
          idleAuxiliarySurface
        ) : threadHeadMessage ? (
          (() => {
            const panel = (
              <MessageQuoteScope value={composerQuotes.threadScope}>
                <MessageThreadPanel
                  channel={activeChannel}
                  onOpenGoalLink={onOpenGoalLink}
                  channelId={activeChannel?.id ?? null}
                  channelName={
                    activeChannel?.name ?? t("channels.pane.groupFallback")
                  }
                  currentPubkey={currentPubkey}
                  disabled={isComposerDisabled || Boolean(readOnlyFooter)}
                  editTarget={threadEditTarget}
                  firstUnreadReplyId={threadFirstUnreadReplyId}
                  huddleMemberPubkeys={huddleMemberPubkeys}
                  huddleMemberPubkeysPending={huddleMemberPubkeysPending}
                  isHuddleTranscript={isHuddleTranscript}
                  isFollowingThread={isFollowingThread}
                  readOnly={Boolean(readOnlyFooter)}
                  threadHeaderActions={!readOnlyFooter}
                  isMessageUnreadById={isMessageUnreadById}
                  isSending={isSending}
                  {...threadLayoutProps}
                  autoSendDraftKey={autoSendDraftKey}
                  onAutoSubmitComplete={handleAutoSubmitComplete}
                  onCancelEdit={onCancelEdit}
                  onCancelReply={onCancelThreadReply}
                  onClose={onCloseThread}
                  onDelete={onDelete}
                  onEdit={handleRoutedEdit}
                  onEditLastOwnMessage={handleEditLastOwnThreadMessage}
                  onEditSave={onEditSave}
                  onFollowThread={onFollowThread}
                  onMarkUnread={onMarkUnread}
                  onMarkRead={onMarkRead}
                  onExpandReplies={onExpandThreadReplies}
                  onSelectReplyTarget={onSelectThreadReplyTarget}
                  onPreviewSend={composerQuotes.previewThread}
                  onSend={composerQuotes.sendThread}
                  onSendToChannel={
                    isComposerDisabled ? undefined : onSendToChannel
                  }
                  onScrollTargetResolved={() =>
                    threadOpen.targetId && !layoutScrollTargetId
                      ? threadOpen.release()
                      : resolveScrollTarget()
                  }
                  onScrollTargetSettled={resolveScrollTarget}
                  onReadingPositionChange={threadOpen.savePosition}
                  onToggleReaction={onToggleReaction}
                  onUnfollowThread={onUnfollowThread}
                  {...{ profiles, recentMentionPubkeys: recentMentions }}
                  replyTargetMessage={threadReplyTargetMessage}
                  scrollTargetHighlights={
                    !layoutScrollTargetId && !threadOpen.targetId
                  }
                  scrollTargetId={
                    layoutScrollTargetId ??
                    threadScrollTargetId ??
                    threadOpen.targetId
                  }
                  scrollTargetTopPx={
                    layoutScrollTargetId ? null : threadOpen.topPx
                  }
                  {...searchHighlightProps.thread}
                  threadHead={threadHeadMessage}
                  videoReviewPresentation={threadVideoReviewPresentation}
                  widthPx={threadPanelWidthPx}
                  threadReplies={threadMessages}
                  threadRepliesPending={
                    threadMessagesPending || threadOpen.isResolving
                  }
                  threadRepliesError={threadMessagesError}
                  onRetryThreadReplies={onRetryThreadReplies}
                  hasOlderReplies={threadHasOlderReplies}
                  isFetchingOlderReplies={threadIsFetchingOlderReplies}
                  onFetchOlderReplies={onFetchOlderThreadReplies}
                  onShowInChannel={onShowThreadInChannel}
                  threadUnreadCount={threadUnreadCounts?.get(
                    threadHeadMessage.id,
                  )}
                  threadReplyUnreadCounts={threadReplyUnreadCounts}
                  threadTypingPubkeys={threadTypingPubkeys}
                  activityAccessoryVisible={hasThreadComposerBotActivity}
                  activityAccessoryContent={
                    hasThreadComposerBotActivity ? (
                      <BotActivityComposerAction
                        agents={activityAgents}
                        channelId={activeChannel?.id ?? null}
                        onOpenAgentSession={onOpenAgentSession}
                        openAgentSessionPubkey={openAgentSessionPubkey}
                        profiles={profiles}
                        workingBotPubkeys={threadComposerBotTypingPubkeys}
                        variant="inline"
                      />
                    ) : null
                  }
                />
              </MessageQuoteScope>
            );
            return wrapThreadPanel(panel);
          })()
        ) : shouldShowThreadSkeleton ? (
          (() => {
            if (isHuddleTranscript) {
              return wrapThreadPanel(<HuddleStartingView />);
            }
            const panel = (
              <MessageThreadPanelSkeleton
                {...threadLayoutProps}
                onClose={onCloseThread}
                widthPx={threadPanelWidthPx}
              />
            );
            return wrapThreadPanel(panel);
          })()
        ) : activeChannel && selectedAgent ? (
          (() => {
            const effectiveAgentSessionChannelId =
              openAgentSessionChannelId &&
              activeChannel.id !== openAgentSessionChannelId
                ? activeChannelId
                : openAgentSessionChannelId;
            const panel = (
              <AgentSessionThreadPanel
                agent={selectedAgent}
                canInterruptTurn={selectedAgent.canInterruptTurn}
                channel={
                  effectiveAgentSessionChannelId
                    ? effectiveAgentSessionChannelId === activeChannel.id
                      ? activeChannel
                      : null
                    : agentSessionSelection.isAgentInActivityList({
                          activityAgents,
                          selectedAgent,
                        })
                      ? activeChannel
                      : null
                }
                channelId={effectiveAgentSessionChannelId}
                isSinglePanelView={
                  useSplitAuxiliaryPane ? false : isSinglePanelView
                }
                layout={useSplitAuxiliaryPane ? "split" : "standalone"}
                transparentChrome={useSplitAuxiliaryPane}
                profiles={profiles}
                onBack={onBackFromAgentSession}
                onClose={onCloseAgentSession}
                widthPx={threadPanelWidthPx}
              />
            );
            return wrapAux(panel, "agent-session-thread-panel");
          })()
        ) : profilePanelPubkey ? (
          (() => {
            const panel = (
              <UserProfilePanel
                currentPubkey={currentPubkey}
                isSinglePanelView={
                  useSplitAuxiliaryPane ? false : isSinglePanelView
                }
                layout={useSplitAuxiliaryPane ? "split" : "standalone"}
                transparentChrome={useSplitAuxiliaryPane}
                onClose={onCloseProfilePanel}
                onOpenDm={onOpenDm}
                onOpenProfile={onOpenProfilePanel}
                onTabChange={onProfilePanelTabChange}
                onViewChange={onProfilePanelViewChange}
                pubkey={profilePanelPubkey}
                splitPaneClamp
                tab={profilePanelTab}
                view={profilePanelView}
                widthPx={threadPanelWidthPx}
              />
            );
            return wrapAux(panel, "user-profile-panel");
          })()
        ) : (
          idleAuxiliarySurface
        )}
      </AnimatePresence>
      <AnimatePresence onExitComplete={threadSurface.markExitComplete}>
        {showIdleAuxiliaryOverThread ? idleAuxiliarySurface : null}
      </AnimatePresence>
    </div>
  );
});
