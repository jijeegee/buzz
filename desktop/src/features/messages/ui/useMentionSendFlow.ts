import { t } from "@/shared/i18n";
import * as React from "react";
import { claimDraftSend } from "@/features/messages/lib/useDrafts";
import { toast } from "sonner";
import { invitationSentText } from "@/shared/api/groupAddOutcome";
import {
  type CreateChannelManagedAgentInput,
  useAttachManagedAgentToChannelMutation,
  useAvailableAcpRuntimes,
  useCreateChannelManagedAgentMutation,
  useManagedAgentsQuery,
  usePersonasQuery,
  useProvisionChannelManagedAgentMutation,
} from "@/features/agents/hooks";
import { resolvePersonaRuntime } from "@/features/agents/lib/resolvePersonaRuntime";
import { useAddChannelMembersMutation } from "@/features/channels/hooks";
import { useCanAddChannelMembers } from "@/features/channels/useCanAddChannelMembers";
import { useNonMemberInvite } from "./useNonMemberInvite";
import { useIsGroupChatChannel } from "@/features/channels/useIsGroupChatChannel";
import { dmThreadAgentMentionError } from "@/features/messages/lib/dmThreadAgentMentionError";
import {
  prepareBackgroundMediaUpload,
  saveQueuedAttachmentsForDraft,
} from "@/features/messages/lib/backgroundMediaUploadStore";
import {
  buildOutgoingMessage,
  type ImetaMedia,
  mergeOutgoingTags,
} from "@/features/messages/lib/imetaMediaMarkdown";
import {
  hasFailedSend,
  replaceFailedSendRetry,
} from "@/features/messages/lib/failedSends";
import { useActivePreparedLinkPreviews } from "./useActivePreparedLinkPreviews";
import { useDetachedAgentStart } from "./useDetachedAgentStart";
import { useEnsureAgentMentionsReady } from "./useEnsureAgentMentionsReady";
import { syncAgentsToActiveHuddle } from "@/shared/api/tauriChannels";
import type { AcpRuntime, ManagedAgent } from "@/shared/api/types";
import { normalizePubkey } from "@/shared/lib/pubkey";
import { buildCustomEmojiTags } from "@/shared/lib/customEmojiTags";
import {
  dedupeQueuedAgentWakes,
  enqueueAgentWake,
  formatMessageSendError,
  getErrorMessage,
  withoutInvitingRecipients,
  mergeMentionRecipients,
  splitSoftAgentMentions,
  type PendingNonMemberMentionSend,
  type QueuedAgentWake,
  type SendMessageWithMentionFlowInput,
  resolvePreviewTags,
  uniqueNormalizedPubkeys,
} from "./useMentionSendFlow.helpers";
import {
  AUTO_ROUTE_MENTION_MARKER,
  buildAgentAddressMentionTags,
  buildSoftMentionTags,
  SOFT_MENTION_MARKER,
} from "@/features/messages/lib/agentAddressMention.mjs";
import { editMessage } from "@/shared/api/editMessage";
import type { AutoRouteDeliver } from "./useAutoAssign";
import { AgentMentionAuthorizationError } from "@/features/messages/lib/agentMentionRevalidation";
import type { UseMentionSendFlowOptions } from "./useMentionSendFlow.types";

export function useMentionSendFlow({
  channelId,
  effectiveDraftKey,
  getComposerRevision,
  runComposerUpdate,
  channelLinks,
  channelType,
  contentRef,
  customEmoji,
  drafts,
  emojiAutocomplete,
  mentions,
  onPrepareSendChannel,
  sendStaysInChannel,
  onAddressedAgentsComposerCleared,
  onAddressedAgentsSendFailed,
  onAddressedAgentsSendSucceeded,
  onSendRef,
  onPreviewSendRef,
  richText,
  setContent,
  setIsEmojiPickerOpen,
  setPendingImeta,
  hasUnsavedMedia,
  clearQueuedAttachments,
  restoreQueuedAttachments,
  setSpoileredAttachmentUrls,
}: UseMentionSendFlowOptions) {
  const [pendingNonMemberSend, setPendingNonMemberSend] =
    React.useState<PendingNonMemberMentionSend | null>(null);
  const [nonMemberPromptError, setNonMemberPromptError] = React.useState<
    string | null
  >(null);
  const [isMentionSendPending, setIsMentionSendPending] = React.useState(false);
  // Persistence identity is independent of the host component and destination
  // channel. A -> B -> A must not revive A's previous invitation or recovery.
  const sourceOwner = React.useMemo(
    () => ({ channelId, draftKey: effectiveDraftKey, getComposerRevision }),
    [channelId, effectiveDraftKey, getComposerRevision],
  );
  const sourceOwnerRef = React.useRef(sourceOwner);
  sourceOwnerRef.current = sourceOwner;
  const [completeSendOwner, setCompleteSendOwner] = React.useState<
    typeof sourceOwner | null
  >(null);
  const isCompleteSendPending = completeSendOwner === sourceOwner;
  const isMentionSendPendingRef = React.useRef(false);
  const completeSendRef = React.useRef<PendingNonMemberMentionSend | null>(
    null,
  );
  const isMountedRef = React.useRef(false);
  const activePreparedLinkPreviews = useActivePreparedLinkPreviews();

  React.useEffect(() => {
    isMountedRef.current = true;
    return () => {
      isMountedRef.current = false;
    };
  }, []);
  const addMembersMutation = useAddChannelMembersMutation(channelId);
  const canInviteNonMembers = useCanAddChannelMembers(channelId);
  const isGroupChatChannel = useIsGroupChatChannel(channelId);
  const attachAgentMutation = useAttachManagedAgentToChannelMutation(channelId);
  const createPersonaAgentMutation =
    useCreateChannelManagedAgentMutation(channelId);
  const provisionPersonaAgentMutation =
    useProvisionChannelManagedAgentMutation(channelId);
  const availableRuntimesQuery = useAvailableAcpRuntimes();
  const managedAgentsQuery = useManagedAgentsQuery();
  const personasQuery = usePersonasQuery();
  // Detached (publish-first) agent wake, bound to the community and identity
  // active at this render so a start that outlives a community switch fails
  // closed instead of spawning against the new tenant. The send path never
  // calls it while preparing a message: wakes are queued during preparation
  // and flushed through this callback only after the relay accepts the
  // publish, so a start failure can never toast "your message was sent"
  // before the publish outcome is known.
  const startAgentDetached = useDetachedAgentStart();
  const getManagedAgentsByPubkey = React.useCallback(async () => {
    const agents =
      managedAgentsQuery.data ??
      (await managedAgentsQuery.refetch()).data ??
      [];
    return new Map(
      agents.map((agent) => [normalizePubkey(agent.pubkey), agent]),
    );
  }, [managedAgentsQuery.data, managedAgentsQuery.refetch]);
  const getPersonas = React.useCallback(async () => {
    return personasQuery.data ?? (await personasQuery.refetch()).data ?? [];
  }, [personasQuery.data, personasQuery.refetch]);
  const getAvailableRuntimes = React.useCallback(async (): Promise<
    AcpRuntime[]
  > => {
    const cached = availableRuntimesQuery.data ?? [];
    if (cached.length > 0 || !availableRuntimesQuery.isLoading) {
      return cached;
    }
    const refetched = await availableRuntimesQuery.refetch();
    return (refetched.data ?? []).filter(
      (runtime): runtime is AcpRuntime =>
        runtime.availability === "available" &&
        runtime.command !== null &&
        runtime.binaryPath !== null,
    );
  }, [
    availableRuntimesQuery.data,
    availableRuntimesQuery.isLoading,
    availableRuntimesQuery.refetch,
  ]);
  const ensureManagedAgentMentionsReady = useEnsureAgentMentionsReady({
    attachAgentToChannel: attachAgentMutation.mutateAsync,
    getManagedAgentsByPubkey,
    getPersonas,
    memberPubkeys: mentions.memberPubkeys,
  });
  const createMentionedPersonaAgents = React.useCallback(
    async (
      personaMentions: ReturnType<typeof mentions.extractMentionPersonas>,
      capturedChannelId: string,
    ) => {
      if (!capturedChannelId || personaMentions.length === 0) {
        return {
          errors: [] as string[],
          agents: [] as ManagedAgent[],
          pubkeys: [] as string[],
          agentsToWake: [] as QueuedAgentWake[],
          mentionRefs: [] as PendingNonMemberMentionSend["savedMentionRefs"],
        };
      }
      const runtimes = await getAvailableRuntimes();
      const defaultRuntime = runtimes[0] ?? null;
      const errors: string[] = [];
      const agents: ManagedAgent[] = [];
      const pubkeys: string[] = [];
      // Queued, not fired: the wakes ride the pending draft and flush only
      // after the publish succeeds, so a persona created for a send the
      // non-member prompt later cancels never wakes at all.
      const agentsToWake: QueuedAgentWake[] = [];
      const mentionRefs: PendingNonMemberMentionSend["savedMentionRefs"] = [];
      const seenPersonaIds = new Set<string>();
      const shouldProvisionForDm =
        channelType === "dm" && Boolean(onPrepareSendChannel);
      for (const { displayName, persona } of personaMentions) {
        if (seenPersonaIds.has(persona.id)) {
          continue;
        }
        seenPersonaIds.add(persona.id);
        const { runtime } = resolvePersonaRuntime(
          persona.runtime,
          runtimes,
          defaultRuntime,
        );
        if (!runtime) {
          errors.push(
            `${displayName}: ${t("messages.agentPrepare.noRuntime")}`,
          );
          continue;
        }
        try {
          const input: CreateChannelManagedAgentInput & {
            channelId: string;
          } = {
            channelId: capturedChannelId,
            runtime,
            name: persona.displayName,
            personaId: persona.id,
            systemPrompt: persona.systemPrompt,
            avatarUrl: persona.avatarUrl ?? undefined,
            model: persona.model ?? undefined,
            role: "bot",
            ensureRunning: true,
            detachedStart: (agentToWake) =>
              enqueueAgentWake(agentsToWake, agentToWake),
          };
          const result = shouldProvisionForDm
            ? await provisionPersonaAgentMutation.mutateAsync(input)
            : await createPersonaAgentMutation.mutateAsync(input);
          if ("remoteAgent" in result) {
            // Runs on another of the owner's computers: already in the
            // channel, nothing to start here.
            const pubkey = normalizePubkey(result.remoteAgent.pubkey);
            pubkeys.push(pubkey);
            mentionRefs.push({ displayName, pubkey, isAgent: true });
            continue;
          }
          const pubkey = normalizePubkey(result.agent.pubkey);
          agents.push(result.agent);
          pubkeys.push(pubkey);
          mentionRefs.push({ displayName, pubkey, isAgent: true });
        } catch (error) {
          errors.push(
            `${displayName}: ${getErrorMessage(
              error,
              t("messages.agentPrepare.createFailed"),
            )}`,
          );
        }
      }
      return {
        agents,
        errors,
        pubkeys: uniqueNormalizedPubkeys(pubkeys),
        agentsToWake,
        mentionRefs,
      };
    },
    [
      createPersonaAgentMutation,
      channelType,
      getAvailableRuntimes,
      onPrepareSendChannel,
      provisionPersonaAgentMutation,
    ],
  );
  const clearComposer = React.useCallback(() => {
    setPendingNonMemberSend(null);
    setNonMemberPromptError(null);
    setContent("");
    contentRef.current = "";
    richText.clearContent();
    setPendingImeta([]);
    clearQueuedAttachments();
    setSpoileredAttachmentUrls?.(new Set());
    mentions.clearMentions();
    channelLinks.clearChannels();
    emojiAutocomplete.clearEmojis();
    setIsEmojiPickerOpen(false);
  }, [
    channelLinks.clearChannels,
    contentRef,
    emojiAutocomplete.clearEmojis,
    mentions.clearMentions,
    richText.clearContent,
    setContent,
    setIsEmojiPickerOpen,
    setPendingImeta,
    clearQueuedAttachments,
    setSpoileredAttachmentUrls,
  ]);
  React.useLayoutEffect(() => {
    setPendingNonMemberSend((draft) =>
      draft?.sourceOwner === sourceOwner ? draft : null,
    );
    setNonMemberPromptError(null);
  }, [sourceOwner]);
  const completeSend = React.useCallback(
    async (
      draft: PendingNonMemberMentionSend,
      mentionPubkeys: string[],
      outgoingTags = draft.outgoingTags,
    ) => {
      const pending = completeSendRef.current;
      if (
        pending?.sourceOwner === draft.sourceOwner &&
        !pending.invitationSignal?.aborted
      )
        return;
      const ownsComposer = () => sourceOwnerRef.current === draft.sourceOwner;
      const sendSignal = draft.preparedLinkPreviews?.signal;
      const isSendCancelled = () =>
        sendSignal?.aborted === true ||
        draft.invitationSignal?.aborted === true;
      if (isSendCancelled()) return draft.preparedLinkPreviews?.release();
      completeSendRef.current = draft;
      setCompleteSendOwner(draft.sourceOwner);
      const preparedUpload =
        draft.queuedAttachments.length > 0
          ? prepareBackgroundMediaUpload(draft.queuedAttachments)
          : null;
      const persistRecoverableDraft = () => {
        // Invitation cancellation still owes the captured draft recovery. Link
        // preview cancellation retains its existing independent recovery owner.
        if (
          sendSignal?.aborted ||
          !draft.recoveryDraftKey ||
          draft.sourceOwner.getComposerRevision() !== draft.composerRevision
        )
          return false;
        const existing = drafts.loadDraft(draft.recoveryDraftKey);
        if (
          existing &&
          (existing.content !== draft.savedContent ||
            existing.channelId !==
              (draft.capturedChannelId ?? draft.recoveryDraftKey) ||
            JSON.stringify(existing.pendingImeta) !==
              JSON.stringify(draft.savedImeta) ||
            JSON.stringify(existing.spoileredAttachmentUrls) !==
              JSON.stringify([...draft.savedSpoileredAttachmentUrls]) ||
            JSON.stringify(existing.mentionRefs ?? []) !==
              JSON.stringify(draft.savedMentionRefs))
        ) {
          return false;
        }
        drafts.persistDraft(
          draft.recoveryDraftKey,
          draft.savedContent,
          draft.capturedChannelId ?? draft.recoveryDraftKey,
          draft.savedImeta,
          [...draft.savedSpoileredAttachmentUrls],
          draft.savedMentionRefs,
        );
        return true;
      };
      const persistPreflightDraft = () => {
        if (
          !draft.recoveryDraftKey ||
          isSendCancelled() ||
          !persistRecoverableDraft()
        )
          return;
        saveQueuedAttachmentsForDraft(
          draft.recoveryDraftKey,
          draft.queuedAttachments,
        );
      };
      let composerCleared = false;
      let optimisticComposerContent = "";
      let clearedRevision = -1;
      const restoreComposerAfterFailure = () => {
        // The failed row keeps the message with Retry / Delete; restoring the
        // draft too would offer it twice.
        if (previewHandedOff && preview && hasFailedSend(preview.localKey))
          return;
        if (!composerCleared) return;
        composerCleared = false;
        // An authored edit (even edit -> clear) ends optimistic recovery's
        // authority over this key, including its persisted record and files.
        if (draft.sourceOwner.getComposerRevision() !== clearedRevision) return;
        const persisted = persistRecoverableDraft();
        const canAnimateCurrentComposer =
          isMountedRef.current && ownsComposer();
        if (
          canAnimateCurrentComposer &&
          draft.addressedAgentPubkeys.length > 0
        ) {
          onAddressedAgentsSendFailed?.(draft.addressedAgentPubkeys);
        }
        const canRestoreCurrentComposer =
          canAnimateCurrentComposer &&
          getComposerRevision() === clearedRevision &&
          contentRef.current.trim() === optimisticComposerContent.trim() &&
          !hasUnsavedMedia();
        if (!canRestoreCurrentComposer && persisted && draft.recoveryDraftKey) {
          saveQueuedAttachmentsForDraft(
            draft.recoveryDraftKey,
            draft.queuedAttachments,
          );
        }
        if (!canRestoreCurrentComposer) {
          return;
        }
        runComposerUpdate(() => {
          setContent(draft.savedContent);
          contentRef.current = draft.savedContent;
          richText.setContent(draft.savedContent);
          setPendingImeta(draft.savedImeta);
          restoreQueuedAttachments(draft.queuedAttachments);
          mentions.restoreDraftMentionRefs(draft.savedMentionRefs);
          setSpoileredAttachmentUrls?.(
            new Set(draft.savedSpoileredAttachmentUrls),
          );
        });
      };
      if (ownsComposer() && getComposerRevision() === draft.composerRevision) {
        runComposerUpdate(() => {
          clearComposer();
          if (draft.addressedAgentPubkeys.length > 0) {
            optimisticComposerContent =
              onAddressedAgentsComposerCleared?.(draft.addressedAgentPubkeys) ??
              "";
            contentRef.current = optimisticComposerContent;
          }
        });
        composerCleared = true;
        clearedRevision = getComposerRevision();
      }
      // Draw the own message now, before preparation and the relay, unless the
      // send can still stop to ask (a 1:1 mention of someone outside it needs
      // a new chat) or must upload first. Mentioning the 1:1's own people
      // (its agent, always addressed there) stays put and draws at once. Abandoned sends discard it in `finally`; a handed-off
      // send keeps it, marked failed, when the relay does not accept it.
      const preview =
        draft.queuedAttachments.length === 0 &&
        !(
          onPrepareSendChannel &&
          mentionPubkeys.length > 0 &&
          !sendStaysInChannel?.(mentionPubkeys)
        )
          ? (() => {
              const { content, mediaTags } = buildOutgoingMessage(
                draft.trimmed,
                draft.savedImeta,
                draft.savedSpoileredAttachmentUrls,
              );
              return (
                onPreviewSendRef?.current?.({
                  channelId: draft.capturedChannelId,
                  content,
                  mentionPubkeys: uniqueNormalizedPubkeys(mentionPubkeys),
                  mediaTags:
                    mergeOutgoingTags(mediaTags, outgoingTags ?? []) ?? [],
                  parentEventId:
                    draft.capturedThreadContext?.parentEventId ?? null,
                }) ?? null
              );
            })()
          : null;
      let previewHandedOff = false;
      // Recover on invalidation, not when an arbitrary external await settles.
      // In particular a later visit to A must load recovery before it can edit
      // or clear A again. The async continuation only observes cancellation.
      const cancelSend = () => {
        restoreComposerAfterFailure();
        if (completeSendRef.current === draft) {
          completeSendRef.current = null;
          if (isMountedRef.current) setCompleteSendOwner(null);
        }
      };
      draft.invitationSignal?.addEventListener("abort", cancelSend, {
        once: true,
      });
      let uploadStarted = false;
      try {
        // Mentions were bound to exact identities when picked; the relay and
        // the agent's runner decide what a mention may do. Sending does not
        // wait on a second client-side check.
        const admittedMentionPubkeys = uniqueNormalizedPubkeys(mentionPubkeys);
        const admittedMentionPubkeySet = new Set(admittedMentionPubkeys);
        const readyAgentPubkeys = new Set(
          uniqueNormalizedPubkeys(draft.readyAgentPubkeys ?? []).filter(
            (pubkey) => admittedMentionPubkeySet.has(pubkey),
          ),
        );
        const managedAgentsByPubkey = await getManagedAgentsByPubkey().catch(
          () => new Map<string, ManagedAgent>(),
        );
        if (isSendCancelled()) return restoreComposerAfterFailure();
        if (!isMountedRef.current) {
          persistPreflightDraft();
          return;
        }
        for (const agent of draft.preparedManagedAgents ?? []) {
          managedAgentsByPubkey.set(normalizePubkey(agent.pubkey), agent);
        }
        const normalizedMentionPubkeys = admittedMentionPubkeys;
        const managedMentionPubkeys = normalizedMentionPubkeys.filter(
          (pubkey) => managedAgentsByPubkey.has(pubkey),
        );
        const agentMentionPubkeys = uniqueNormalizedPubkeys([
          ...managedMentionPubkeys,
          ...normalizedMentionPubkeys.filter(mentions.isAgentPubkey),
        ]);
        const agentMentionPubkeySet = new Set(agentMentionPubkeys);
        const preparedAgentPubkeys = uniqueNormalizedPubkeys([
          ...readyAgentPubkeys,
          ...agentMentionPubkeys,
        ]);
        let sendChannelId = draft.capturedChannelId;
        // In a 1:1 every mentioned person (not only agents) goes through the
        // preparer, which asks before starting a new chat with them.
        const preparePubkeys =
          channelType === "dm"
            ? uniqueNormalizedPubkeys([
                ...preparedAgentPubkeys,
                ...normalizedMentionPubkeys,
              ])
            : preparedAgentPubkeys;
        if (preparePubkeys.length > 0 && onPrepareSendChannel) {
          sendChannelId = await onPrepareSendChannel(preparePubkeys);
          if (isSendCancelled()) return restoreComposerAfterFailure();
          if (!sendChannelId) {
            return restoreComposerAfterFailure();
          }
          if (!isMountedRef.current) {
            persistPreflightDraft();
            return;
          }
        }
        // Sending neither starts a stopped agent nor adds one to the group: a
        // mentioned agent that is running and a member answers, others don't.
        // Only wakes queued by persona creation on this draft remain; they
        // flush after the relay accepts the publish, so an aborted send drops
        // them.
        const agentsToWake = dedupeQueuedAgentWakes(
          draft.queuedAgentWakes ?? [],
        );
        if (preparedAgentPubkeys.length > 0 && sendChannelId) {
          try {
            await syncAgentsToActiveHuddle(sendChannelId, preparedAgentPubkeys);
            if (isSendCancelled()) return restoreComposerAfterFailure();
          } catch (error) {
            if (isSendCancelled()) return restoreComposerAfterFailure();
            const message = t("messages.send.huddleAddFailed", {
              error: getErrorMessage(error, t("messages.send.huddleFallback")),
            });
            setNonMemberPromptError(message);
            toast.error(message);
            return restoreComposerAfterFailure();
          }
        }
        const send = onSendRef.current;
        const finishSend = async (
          uploaded: ImetaMedia[],
          signal?: AbortSignal,
        ) => {
          const { content: finalContent, mediaTags } = buildOutgoingMessage(
            draft.trimmed,
            [...draft.savedImeta, ...uploaded],
            new Set([
              ...draft.savedSpoileredAttachmentUrls,
              ...draft.queuedAttachments.flatMap((attachment, index) =>
                attachment.spoilered && uploaded[index]
                  ? [uploaded[index].url]
                  : [],
              ),
            ]),
          );
          const finalOutgoingTags = await resolvePreviewTags(
            draft,
            mediaTags,
            outgoingTags,
          );
          if (!finalOutgoingTags || signal?.aborted || isSendCancelled())
            return restoreComposerAfterFailure();
          const revalidatedMentionPubkeys = admittedMentionPubkeys;
          const { delivered: deliveredMentionPubkeys, soft: softPubkeys } =
            draft.softAgentMentions
              ? splitSoftAgentMentions(
                  revalidatedMentionPubkeys,
                  (pubkey) =>
                    agentMentionPubkeySet.has(pubkey) ||
                    mentions.isAgentPubkey(pubkey),
                  [...draft.addressedAgentPubkeys, ...readyAgentPubkeys],
                )
              : { delivered: revalidatedMentionPubkeys, soft: [] };
          const finalTagsWithAgentAddress = [
            ...finalOutgoingTags,
            ...buildAgentAddressMentionTags(
              draft.addressedAgentPubkeys,
              deliveredMentionPubkeys,
            ),
            ...buildSoftMentionTags(softPubkeys),
          ];
          // A preview drawn for another group (a 1:1 can move the send to a
          // new chat) is not reused.
          const previewKey =
            preview && sendChannelId === draft.capturedChannelId
              ? preview.localKey
              : undefined;
          previewHandedOff = previewKey !== undefined;
          const publish = async (): Promise<unknown> => {
            try {
              const published = await send(
                finalContent,
                deliveredMentionPubkeys,
                finalTagsWithAgentAddress,
                sendChannelId,
                draft.capturedThreadContext,
                draft.preparedLinkPreviews != null,
                previewKey,
              );
              // The relay accepted the publish: flush the queued wakes now,
              // before the post-send cancellation check — a cancellation
              // racing a successful publish must not drop the wake for a
              // message that did land. Fire-and-forget: the send awaits
              // nothing here, and each wake carries its enqueue-time replay
              // floor so the spawned harness replays back past this message
              // however late the flush.
              for (const wake of agentsToWake) {
                startAgentDetached(wake.agent, wake.replayFloorUnix);
              }
              if (published) draft.onPublished?.(published);
              return published;
            } catch (error) {
              // The failed row's Retry publishes again with the same
              // after-publish steps (wakes, routing) as this send.
              if (previewKey) {
                replaceFailedSendRetry(previewKey, () => {
                  publish().catch(() => {});
                });
              }
              throw error;
            }
          };
          await publish();
          if (signal?.aborted || isSendCancelled()) return;
          const sentMentionPubkeys = new Set(
            deliveredMentionPubkeys.map(normalizePubkey),
          );
          const newlyPinnedPubkeys = draft.inlineAgentMentionPubkeys.filter(
            (pubkey) => sentMentionPubkeys.has(normalizePubkey(pubkey)),
          );
          if (
            ownsComposer() &&
            getComposerRevision() === draft.composerRevision
          ) {
            onAddressedAgentsSendSucceeded?.(
              [
                ...new Set([
                  ...draft.addressedAgentPubkeys,
                  ...newlyPinnedPubkeys,
                ]),
              ],
              newlyPinnedPubkeys,
            );
          }
          if (
            draft.sentDraftKey &&
            draft.sourceOwner.getComposerRevision() ===
              draft.composerRevision &&
            JSON.stringify(
              drafts.loadDraft(draft.sentDraftKey)?.mentionRefs ?? [],
            ) === JSON.stringify(draft.savedMentionRefs)
          ) {
            drafts.markDraftSent(
              draft.sentDraftKey,
              draft.savedContent,
              draft.capturedChannelId ?? draft.sentDraftKey,
              draft.savedImeta,
              [...draft.savedSpoileredAttachmentUrls],
            );
          }
        };
        if (preparedUpload) {
          let settleUpload!: () => void;
          const uploadSettled = new Promise<void>((resolve) => {
            settleUpload = resolve;
          });
          uploadStarted = preparedUpload.start({
            onComplete: async (uploaded, signal) => {
              try {
                await finishSend(uploaded, signal);
              } catch (error) {
                restoreComposerAfterFailure();
                toast.error(
                  error instanceof AgentMentionAuthorizationError
                    ? error.message
                    : formatMessageSendError(error),
                );
              } finally {
                settleUpload();
              }
            },
            onError: (error) => {
              restoreComposerAfterFailure();
              toast.error(
                t("messages.send.uploadFailed", {
                  error: getErrorMessage(error, t("messages.errors.unknown")),
                }),
              );
              settleUpload();
            },
            onCancel: () => {
              restoreComposerAfterFailure();
              settleUpload();
            },
          });
          if (!uploadStarted) {
            settleUpload();
            return restoreComposerAfterFailure();
          }
          await uploadSettled;
        }
        if (!preparedUpload) {
          try {
            await finishSend([]);
          } catch (error) {
            restoreComposerAfterFailure();
            toast.error(
              error instanceof AgentMentionAuthorizationError
                ? error.message
                : formatMessageSendError(error),
            );
          }
        }
      } catch (error) {
        restoreComposerAfterFailure();
        toast.error(getErrorMessage(error, t("messages.send.retryFailed")));
      } finally {
        if (preview && !previewHandedOff) preview.discard();
        if (draft.preparedLinkPreviews) {
          activePreparedLinkPreviews.delete(draft.preparedLinkPreviews);
        }
        draft.preparedLinkPreviews?.release();
        if (!uploadStarted) preparedUpload?.cancel();
        draft.invitationSignal?.removeEventListener("abort", cancelSend);
        if (completeSendRef.current === draft) {
          completeSendRef.current = null;
          if (isMountedRef.current) setCompleteSendOwner(null);
        }
      }
    },
    [
      channelType,
      clearComposer,
      getComposerRevision,
      runComposerUpdate,
      contentRef,
      drafts,
      getManagedAgentsByPubkey,
      mentions.isAgentPubkey,
      onAddressedAgentsComposerCleared,
      onAddressedAgentsSendFailed,
      onAddressedAgentsSendSucceeded,
      onPrepareSendChannel,
      onPreviewSendRef,
      onSendRef,
      sendStaysInChannel,
      richText.setContent,
      setContent,
      startAgentDetached,
      setPendingImeta,
      restoreQueuedAttachments,
      setSpoileredAttachmentUrls,
      hasUnsavedMedia,
      mentions.restoreDraftMentionRefs,
      activePreparedLinkPreviews,
    ],
  );
  const sendMessageWithMentionFlow = React.useCallback(
    async ({
      addressedAgentPubkeys = [],
      onPublished,
      softAgentMentions = false,
      capturedChannelId,
      capturedThreadContext = null,
      pendingImeta,
      queuedAttachments = [],
      linkPreviewTags = [],
      preparedLinkPreviews = null,
      sentDraftKey,
      recoveryDraftKey,
      spoileredAttachmentUrls = new Set(),
      trimmed,
    }: SendMessageWithMentionFlowInput) => {
      if (isMentionSendPendingRef.current) {
        return;
      }
      isMentionSendPendingRef.current = true;
      setIsMentionSendPending(true);
      // Bind settlement to this authored visit before reading its recipients.
      claimDraftSend(effectiveDraftKey);
      const composerRevision = getComposerRevision();
      const isSendCancelled = () =>
        preparedLinkPreviews?.signal.aborted === true;
      let sendPromoted = false;
      if (preparedLinkPreviews) {
        activePreparedLinkPreviews.add(preparedLinkPreviews);
      }
      try {
        if (isSendCancelled()) return;
        // Every extraction below reads the mention map, and a pasted identity
        // can still be verifying — the relay round trip for a non-member is
        // exactly the case this feature exists for. Sending first would
        // publish a readable `@Label` with no `p` tag. Bounded inside, so a
        // lookup that never answers delays the send rather than blocking it.
        await mentions.settlePendingMentionBindings();
        // Settlement may outlive an edit or A → B → A navigation. In that
        // case the live mention maps no longer belong to this send.
        if (
          isSendCancelled() ||
          !isMountedRef.current ||
          sourceOwnerRef.current !== sourceOwner ||
          getComposerRevision() !== composerRevision
        )
          return;
        const savedMentionRefs = mentions.getDraftMentionRefs(trimmed).slice();
        const selectedMentionPubkeys = mentions.extractMentionPubkeys(trimmed);
        const selectedPersonas = mentions.extractMentionPersonas(trimmed);
        const dmThreadAgentMentionErrorMessage = dmThreadAgentMentionError({
          trimmed,
          isThreadReply: capturedThreadContext != null,
          channelType,
          extractMentionPersonas: mentions.extractMentionPersonas,
          extractMentionPubkeys: (text) =>
            mergeMentionRecipients(
              mentions.extractMentionPubkeys(text),
              addressedAgentPubkeys,
            ),
          isAgentPubkey: mentions.isAgentPubkey,
          hasResolvedMembers: mentions.hasResolvedMembers,
          memberPubkeys: mentions.memberPubkeys,
        });
        if (dmThreadAgentMentionErrorMessage) {
          setNonMemberPromptError(dmThreadAgentMentionErrorMessage);
          toast.error(dmThreadAgentMentionErrorMessage);
          return;
        }
        let effectiveChannelId = capturedChannelId;
        if (!effectiveChannelId && onPrepareSendChannel) {
          effectiveChannelId = await onPrepareSendChannel();
          if (isSendCancelled()) return;
          if (!effectiveChannelId) {
            return;
          }
        }
        const personaMentionResult = await createMentionedPersonaAgents(
          selectedPersonas,
          effectiveChannelId ?? "",
        );
        if (isSendCancelled()) return;
        if (personaMentionResult.errors.length > 0) {
          const message =
            personaMentionResult.errors.length === 1
              ? t("messages.send.agentMentionFailed", {
                  error: personaMentionResult.errors[0],
                })
              : t("messages.send.agentMentionsFailed", {
                  errors: personaMentionResult.errors.join("; "),
                });
          setNonMemberPromptError(message);
          toast.error(message);
          return;
        }
        const createdPersonaAgentPubkeys = personaMentionResult.pubkeys;
        savedMentionRefs.push(...personaMentionResult.mentionRefs);
        // Preparation resolves the captured persona, never the editor's new
        // selection. Only this unchanged visit may receive its resolved binding.
        if (
          isMountedRef.current &&
          sourceOwnerRef.current === sourceOwner &&
          getComposerRevision() === composerRevision
        ) {
          for (const ref of personaMentionResult.mentionRefs)
            mentions.registerMentionPubkey(ref.displayName, ref.pubkey, ref);
        }
        const explicitMentionPubkeys = uniqueNormalizedPubkeys([
          ...selectedMentionPubkeys,
          ...createdPersonaAgentPubkeys,
        ]);
        const pubkeys = mergeMentionRecipients(
          explicitMentionPubkeys,
          addressedAgentPubkeys,
        );
        const outgoingTags = [
          ...buildCustomEmojiTags(trimmed, customEmoji),
          ...linkPreviewTags,
        ];
        const pendingDraft: PendingNonMemberMentionSend = {
          sourceOwner,
          composerRevision,
          addressedAgentPubkeys: uniqueNormalizedPubkeys(addressedAgentPubkeys),
          onPublished,
          softAgentMentions,
          inlineAgentMentionPubkeys: uniqueNormalizedPubkeys(
            savedMentionRefs
              .filter((ref) => ref.isAgent)
              .map((ref) => ref.pubkey),
          ),
          capturedChannelId: effectiveChannelId,
          capturedThreadContext,
          trimmed,
          mentionPubkeys: pubkeys,
          // Mentions go out as written: Kuzz never asks to add anyone to the
          // room on send, and a mention of a non-member simply doesn't reach
          // them.
          nonMemberPubkeys: [],
          outgoingTags,
          preparedLinkPreviews,
          preparedManagedAgents: personaMentionResult.agents,
          queuedAgentWakes: personaMentionResult.agentsToWake,
          readyAgentPubkeys:
            channelType === "dm" && onPrepareSendChannel
              ? []
              : createdPersonaAgentPubkeys,
          savedContent: trimmed,
          savedImeta: [...pendingImeta],
          queuedAttachments: [...queuedAttachments],
          savedSpoileredAttachmentUrls: new Set(spoileredAttachmentUrls),
          sentDraftKey,
          recoveryDraftKey,
          savedMentionRefs,
        };
        sendPromoted = true;
        await completeSend(pendingDraft, pubkeys);
      } catch (error) {
        toast.error(
          getErrorMessage(error, t("messages.send.prepareMentionsFailed")),
        );
      } finally {
        if (!sendPromoted) {
          if (preparedLinkPreviews) {
            activePreparedLinkPreviews.delete(preparedLinkPreviews);
          }
          preparedLinkPreviews?.release();
        }
        isMentionSendPendingRef.current = false;
        setIsMentionSendPending(false);
      }
    },
    [
      completeSend,
      effectiveDraftKey,
      sourceOwner,
      getComposerRevision,
      channelType,
      createMentionedPersonaAgents,
      customEmoji,
      mentions.extractMentionPersonas,
      mentions.extractMentionPubkeys,
      mentions.hasResolvedMembers,
      mentions.isAgentPubkey,
      mentions.memberPubkeys,
      mentions.getDraftMentionRefs,
      mentions.settlePendingMentionBindings,
      mentions.registerMentionPubkey,
      onPrepareSendChannel,
      activePreparedLinkPreviews,
    ],
  );
  const pendingNonMemberNames = React.useMemo(() => {
    if (!pendingNonMemberSend) return [];
    return pendingNonMemberSend.nonMemberPubkeys.map(
      (pubkey) =>
        mentions.getMentionDisplayName(pubkey) ?? t("common.unknownUser"),
    );
  }, [mentions.getMentionDisplayName, pendingNonMemberSend]);
  const invitation = useNonMemberInvite({
    sourceOwner,
    draft: pendingNonMemberSend,
    canInvite: canInviteNonMembers,
    revalidate: mentions.revalidateMentionPubkeys,
    getManagedAgentsByPubkey,
    isAgentPubkey: mentions.isAgentPubkey,
    addMembers: addMembersMutation.mutateAsync,
    completeSend,
    setError: setNonMemberPromptError,
    onInvited: (pubkeys) =>
      toast(
        invitationSentText(
          pubkeys
            .map(
              (pubkey) =>
                mentions.getMentionDisplayName(pubkey) ??
                t("common.unknownUser"),
            )
            .join(", "),
        ),
      ),
  });
  const handleSendWithoutInviting = React.useCallback(() => {
    if (!pendingNonMemberSend) return;
    invitation.cancel();
    const { mentionPubkeys, outgoingTags } =
      withoutInvitingRecipients(pendingNonMemberSend);
    void completeSend(pendingNonMemberSend, mentionPubkeys, outgoingTags);
  }, [completeSend, pendingNonMemberSend, invitation.cancel]);
  const dismissNonMemberPrompt = React.useCallback(() => {
    invitation.cancel();
    setPendingNonMemberSend(null);
    setNonMemberPromptError(null);
  }, [invitation.cancel]);
  // Smart routing's delivery: the pick joins the already-published message
  // through an edit that newly `p`-tags it (the harness wakes an agent on an
  // edit that newly mentions it) plus the `auto-route` display tag. Same
  // body, attachments, and emoji, so the edit changes nothing visible. Wakes
  // flush only after the edit is accepted, like a send's.
  const deliverAutoRoute = React.useCallback<AutoRouteDeliver>(
    async (message, pubkeys, isCurrent, route) => {
      const messageChannelId = message.tags.find((tag) => tag[0] === "h")?.[1];
      if (!messageChannelId) throw new Error("Message has no group.");
      const readiness = await ensureManagedAgentMentionsReady(
        pubkeys,
        messageChannelId,
      );
      if (readiness.errors.length > 0) {
        throw new Error(readiness.errors.join("; "));
      }
      if (!isCurrent()) return false;
      await editMessage(
        messageChannelId,
        message.id,
        message.content,
        message.tags.filter((tag) => tag[0] === "imeta"),
        message.tags.filter((tag) => tag[0] === "emoji"),
        pubkeys,
        false,
        [
          // The edit's mention snapshot replaces the original's body
          // identities, so it repeats them (soft ones included).
          ...uniqueNormalizedPubkeys(
            message.tags.flatMap((tag) =>
              tag[1] &&
              (tag[0] === "p" ||
                (tag[0] === "mention" &&
                  (tag.length === 2 || tag[2] === SOFT_MENTION_MARKER)))
                ? [tag[1]]
                : [],
            ),
          ).map((pubkey) => ["mention", pubkey]),
          ...pubkeys.map((pubkey) => [
            "mention",
            pubkey,
            AUTO_ROUTE_MENTION_MARKER,
          ]),
        ],
        route,
      );
      for (const wake of dedupeQueuedAgentWakes(readiness.agentsToWake)) {
        startAgentDetached(wake.agent, wake.replayFloorUnix);
      }
      return true;
    },
    [ensureManagedAgentMentionsReady, startAgentDetached],
  );
  return {
    deliverAutoRoute,
    // Agent starts are detached (publish-first), so useDetachedAgentStart's
    // in-flight state deliberately does not gate the composer — a background
    // start must not block the next send.
    isPreparingMentionSend:
      invitation.isPending ||
      isMentionSendPending ||
      isCompleteSendPending ||
      attachAgentMutation.isPending ||
      createPersonaAgentMutation.isPending,
    nonMemberPromptProps: {
      canInvite: canInviteNonMembers,
      isChat: isGroupChatChannel,
      error: nonMemberPromptError,
      isInvitePending:
        invitation.isPending ||
        isMentionSendPending ||
        isCompleteSendPending ||
        attachAgentMutation.isPending ||
        createPersonaAgentMutation.isPending,
      names: pendingNonMemberNames,
      onDismiss: dismissNonMemberPrompt,
      onDoNothing: handleSendWithoutInviting,
      onInvite: invitation.invite,
      open: pendingNonMemberSend !== null,
    },
    sendMessageWithMentionFlow,
  };
}
