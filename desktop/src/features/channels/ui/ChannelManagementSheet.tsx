import {
  Archive,
  ArchiveRestore,
  BookOpenText,
  DoorClosed,
  DoorOpen,
  Fingerprint,
  Target,
  Trash2,
  Workflow as WorkflowIcon,
} from "lucide-react";
import * as React from "react";
import * as DialogPrimitive from "@radix-ui/react-dialog";

import {
  useArchiveChannelMutation,
  useCanvasQuery,
  useChannelDetailsQuery,
  useChannelMembersQuery,
  useDeleteChannelMutation,
  useJoinChannelMutation,
  type LeaveChannelArgs,
  useLeaveChannelMutation,
  useUnarchiveChannelMutation,
  useUpdateChannelMutation,
} from "@/features/channels/hooks";
import { canvasIngressOpen } from "./canvasIngress";
import { compareMembersByRole } from "@/features/channels/lib/memberUtils";
import { useAppNavigation } from "@/app/navigation/useAppNavigation";
import { useChannelWorkflowsQuery } from "@/features/workflows/hooks";
import { DEFAULT_EPHEMERAL_TTL_SECONDS } from "@/features/channels/lib/ephemeralChannel";
import type { GoalLinkTarget } from "@/features/goals/goalTree";
import type { Channel, ChannelMember, Workflow } from "@/shared/api/types";
import { useWorkflowEditorOverlay } from "@/shared/context/WorkflowEditorOverlayContext";
import { useFeatureEnabled } from "@/shared/features";
import { cn } from "@/shared/lib/cn";
import { useTheme } from "@/shared/theme/ThemeProvider";
import { Button } from "@/shared/ui/button";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/shared/ui/dialog";
import { Input } from "@/shared/ui/input";
import { Textarea } from "@/shared/ui/textarea";
import {
  AuxiliaryPanelBody,
  AuxiliaryPanelContext,
  AuxiliaryPanelHeader,
  AuxiliaryPanelHeaderGroup,
  AuxiliaryPanelTitle,
  type AuxiliaryPanelMode,
  getAuxiliaryPanelMode,
} from "@/shared/layout/AuxiliaryPanel";
import { useScrollBoundaryLock } from "@/shared/hooks/useScrollBoundaryLock";
import { useDeferredModalOpen } from "@/shared/ui/deferredModalOpen";
import {
  OverlayPanelBackdrop,
  PANEL_BASE_CLASS,
  PANEL_ENTER_MOTION_CLASS,
  PANEL_OVERLAY_CLASS,
} from "@/shared/ui/OverlayPanelBackdrop";
import { KeyedChannelCanvas } from "./KeyedChannelCanvas";
import { GoalsPanel } from "@/features/goals/ui/GoalsPanel";
import { ChannelWorkflowsSection } from "./ChannelWorkflowsSection";
import {
  CHANNEL_FORM_FIELD_CONTROL_CLASS,
  CHANNEL_FORM_FIELD_SHELL_CLASS,
} from "./channelFormStyles";
import {
  ChannelTypeDetailRow,
  ChannelTypeSettings,
} from "./ChannelTypeSettings";
import { ChannelPermissionsSettings } from "./ChannelPermissionsSettings";
import { GroupSharingSettings } from "@/features/groups/ui/GroupSharingSettings";
import { GroupVisibilityConfirmDialog } from "@/features/groups/ui/GroupVisibilityConfirmDialog";
import { GroupPhotoField } from "./GroupPhotoField";
import {
  ActionFieldRow,
  ChannelHero,
  CopyFieldRow,
  EditableInfoFieldRow,
  FieldGroup,
  getMarkdownPreviewText,
  InfoFieldRow,
  IngressRow,
} from "./ChannelManagementSheetRows";
import {
  ChannelDeleteConfirmationDialog,
  useChannelModerationCapabilities,
} from "./ChannelManagementModerationActions";
import { ChannelMemberAvatarStack } from "./ChannelMemberAvatarStack";
import { ChannelGoalsSwitchRow } from "./ChannelGoalsSwitchRow";
import { ChannelHistorySwitchRow } from "./ChannelHistorySwitchRow";
import { useChannelGoalsSwitch } from "./useChannelGoalsSwitch";
import { isGroupChat } from "../lib/groupChat";
import { ChatInfoSummary, MuteNotificationsRow } from "./ChatInfoSummary";
import { useT } from "@/shared/i18n";
import { LeaveChannelConfirmDialog } from "./LeaveChannelConfirmDialog";

type ManagementView = "summary" | "canvas" | "workflows" | "goals";

type ChannelManagementSheetProps = {
  channel: Channel | null;
  animateSplitEnter?: boolean;
  currentPubkey?: string;
  /** View to show when the sheet opens (e.g. straight into goals). */
  initialView?: "goals" | null;
  layout?: "overlay" | "split";
  /** Opens a thread or message linked from the goals view. */
  onOpenGoalLink?: (target: GoalLinkTarget) => void;
  onDeleted?: () => void;
  onOpenMembers?: () => void;
  onOpenChange: (open: boolean) => void;
  open: boolean;
  transparentChrome?: boolean;
};

export function ChannelManagementSheet({
  animateSplitEnter = false,
  channel,
  currentPubkey,
  initialView = null,
  layout = "overlay",
  onDeleted,
  onOpenGoalLink,
  onOpenMembers,
  onOpenChange,
  open,
  transparentChrome = false,
}: ChannelManagementSheetProps) {
  const t = useT();
  const { isDark } = useTheme();
  const { goNewWorkflowForChannel, goWorkflow } = useAppNavigation();
  const {
    openNewWorkflow: openNewWorkflowOverlay,
    openWorkflow: openWorkflowOverlay,
  } = useWorkflowEditorOverlay();
  const isSplitLayout = layout === "split";
  const auxiliaryPanelMode = getAuxiliaryPanelMode(
    isSplitLayout,
    !isSplitLayout,
  );
  const channelId = channel?.id ?? null;
  const workflowsEnabled = useFeatureEnabled("workflows");
  const detailsQuery = useChannelDetailsQuery(channelId, open);
  const membersQuery = useChannelMembersQuery(channelId, open);
  const canvasQuery = useCanvasQuery(channelId, channelId !== null && open);
  const workflowsQuery = useChannelWorkflowsQuery(
    workflowsEnabled && channelId !== null && open ? channelId : null,
  );
  const updateChannelDetailsMutation = useUpdateChannelMutation(channelId);
  const archiveChannelMutation = useArchiveChannelMutation(channelId);
  const unarchiveChannelMutation = useUnarchiveChannelMutation(channelId);
  const deleteChannelMutation = useDeleteChannelMutation(channelId);
  const joinChannelMutation = useJoinChannelMutation(channelId);
  const leaveChannelMutation = useLeaveChannelMutation(channelId);

  const detail = detailsQuery.data ?? channel;
  const members = React.useMemo(() => {
    const currentMembers = membersQuery.data ?? [];
    return [...currentMembers].sort((left, right) =>
      compareMembersByRole(left, right, currentPubkey),
    );
  }, [currentPubkey, membersQuery.data]);
  const selfMember =
    members.find((member) => member.pubkey === currentPubkey) ?? null;
  const hasResolvedMembership = membersQuery.data !== undefined;

  const { canDeleteChannel, canManageChannel } =
    useChannelModerationCapabilities(membersQuery.data, currentPubkey, open);
  // Ownerless chats have no owner/admin controls at all.
  const isChat = isGroupChat(detail);
  const canEditChannel =
    canManageChannel && detail?.channelType !== "dm" && !isChat;
  // Room goal UI (ingress + goals view) follows the per-room switch.
  const goalsSwitch = useChannelGoalsSwitch(detail, canManageChannel);
  const goalsEnabled = goalsSwitch.enabled;
  const canEditNarrative =
    canManageChannel &&
    selfMember !== null &&
    detail?.channelType !== "dm" &&
    !isChat;
  const isArchived =
    detail?.archivedAt !== null && detail?.archivedAt !== undefined;
  const canJoin =
    hasResolvedMembership &&
    detail?.channelType !== "dm" &&
    detail?.visibility === "open" &&
    !isArchived &&
    selfMember === null;
  const canLeave =
    hasResolvedMembership &&
    detail?.channelType !== "dm" &&
    !isArchived &&
    selfMember !== null;
  const memberCount =
    members.length || detail?.memberCount || channel?.memberCount || 0;

  const [nameDraft, setNameDraft] = React.useState("");
  const [descriptionDraft, setDescriptionDraft] = React.useState("");
  const [pictureDraft, setPictureDraft] = React.useState("");
  const [isPhotoUploading, setIsPhotoUploading] = React.useState(false);
  const [isPrivateDraft, setIsPrivateDraft] = React.useState(false);
  const [isEphemeralDraft, setIsEphemeralDraft] = React.useState(false);
  const [ttlSecondsDraft, setTtlSecondsDraft] = React.useState(
    DEFAULT_EPHEMERAL_TTL_SECONDS,
  );
  const [isDeleteDialogOpen, setIsDeleteDialogOpen] = React.useState(false);
  const [isEditDialogOpen, setIsEditDialogOpen] = React.useState(false);
  const [isVisibilityConfirmOpen, setIsVisibilityConfirmOpen] =
    React.useState(false);
  const [hasUserEditedChannelDraft, setHasUserEditedChannelDraft] =
    React.useState(false);
  const [activeView, setActiveView] = React.useState<ManagementView>("summary");
  const visibleActiveView =
    (activeView === "workflows" && (!workflowsEnabled || isChat)) ||
    (activeView === "goals" && !goalsEnabled)
      ? "summary"
      : activeView;
  const openingView: ManagementView = initialView ?? "summary";
  // The goals chip can retarget an already-open sheet.
  React.useEffect(() => {
    if (open && initialView) setActiveView(initialView);
  }, [initialView, open]);
  const { cancelDeferredModalOpen, openNextFrame: openModalNextFrame } =
    useDeferredModalOpen();

  const openEditDialog = React.useCallback(() => {
    setIsEditDialogOpen(false);
    openModalNextFrame(() => setIsEditDialogOpen(true));
  }, [openModalNextFrame]);

  const openMembersDialog = React.useCallback(() => {
    if (!onOpenMembers) return;

    openModalNextFrame(onOpenMembers);
  }, [onOpenMembers, openModalNextFrame]);

  // Sync drafts from server only when the sheet opens or the channel changes -
  // not on every background refetch, which would clobber in-flight edits.
  const syncedForRef = React.useRef<string | null>(null);
  React.useEffect(() => {
    if (!open) {
      // Reset on close so the next open re-syncs from server.
      cancelDeferredModalOpen();
      syncedForRef.current = null;
      setIsDeleteDialogOpen(false);
      setIsEditDialogOpen(false);
      setActiveView("summary");
      return;
    }
    if (!detail) {
      return;
    }

    const key = detail.id;
    if (syncedForRef.current === key) {
      return;
    }
    syncedForRef.current = key;

    setNameDraft(detail.name);
    setDescriptionDraft(detail.description);
    setPictureDraft(detail.picture ?? "");
    setIsPrivateDraft(detail.visibility === "private");
    setIsEphemeralDraft(detail.ttlSeconds !== null);
    setTtlSecondsDraft(detail.ttlSeconds ?? DEFAULT_EPHEMERAL_TTL_SECONDS);
    setHasUserEditedChannelDraft(false);
    setActiveView(openingView);
  }, [cancelDeferredModalOpen, detail, open, openingView]);

  if (!channel) {
    return null;
  }

  function handleDeleteDialogOpenChange(next: boolean) {
    deleteChannelMutation.reset();
    setIsDeleteDialogOpen(next);
  }

  async function handleDeleteChannel() {
    try {
      await deleteChannelMutation.mutateAsync();
      handleDeleteDialogOpenChange(false);
      onOpenChange(false);
      onDeleted?.();
    } catch {
      // The mutation error is rendered inline in the confirmation dialog.
    }
  }

  function handlePanelOpenChange(next: boolean) {
    if (!next) {
      handleDeleteDialogOpenChange(false);
    }

    onOpenChange(next);
  }

  // Workflows open as a modal above the channel settings Workflows view. Keep
  // that view mounted behind the editor so every completed close path (clean,
  // dirty-discard, or create cancel) returns to the exact surface that opened
  // it. The navigation fallbacks still close the sheet before changing routes;
  // canonical /workflows deep links stay unchanged either way.
  function handleOpenWorkflow(workflow: Workflow) {
    if (openWorkflowOverlay) {
      openWorkflowOverlay(workflow.id, workflow);
      return;
    }

    handlePanelOpenChange(false);
    void goWorkflow(workflow.id);
  }

  function handleCreateWorkflow() {
    if (!channelId) return;

    if (openNewWorkflowOverlay) {
      openNewWorkflowOverlay(channelId);
      return;
    }

    handlePanelOpenChange(false);
    void goNewWorkflowForChannel(channelId);
  }

  const currentVisibility = detail?.visibility ?? channel.visibility;
  const currentTtlSeconds = detail?.ttlSeconds ?? null;
  const nextVisibility: "open" | "private" = isPrivateDraft
    ? "private"
    : "open";
  const nextTtlSeconds: number | null = isEphemeralDraft
    ? ttlSecondsDraft
    : null;
  const lifecycleDirty =
    nextVisibility !== currentVisibility ||
    nextTtlSeconds !== currentTtlSeconds;

  const resolvedChannel = detail ?? channel;
  const nameDirty = nameDraft.trim() !== resolvedChannel.name.trim();
  const descriptionDirty =
    descriptionDraft.trim() !== resolvedChannel.description.trim();
  const pictureDirty = pictureDraft !== (resolvedChannel.picture ?? "");
  const isSavingChannelEdits = updateChannelDetailsMutation.isPending;
  const hasChannelEditChanges =
    nameDirty || descriptionDirty || lifecycleDirty || pictureDirty;
  const canSaveChannelEdits =
    nameDraft.trim().length > 0 &&
    hasUserEditedChannelDraft &&
    hasChannelEditChanges &&
    !isPhotoUploading &&
    !isSavingChannelEdits;
  const canvasContent = canvasQuery.data?.content?.trim() ?? "";
  const hasCanvas = canvasContent.length > 0;
  const canvasPreview = hasCanvas
    ? getMarkdownPreviewText(canvasContent)
    : undefined;
  const canOpenCanvas = canvasIngressOpen(
    canvasQuery.data?.eventId,
    canEditNarrative,
  );

  function handleEditDialogOpenChange(next: boolean) {
    if (next) {
      openEditDialog();
      return;
    }

    cancelDeferredModalOpen();
    setNameDraft(resolvedChannel.name);
    setDescriptionDraft(resolvedChannel.description);
    setPictureDraft(resolvedChannel.picture ?? "");
    setIsPrivateDraft(currentVisibility === "private");
    setIsEphemeralDraft(currentTtlSeconds !== null);
    setTtlSecondsDraft(currentTtlSeconds ?? DEFAULT_EPHEMERAL_TTL_SECONDS);
    setHasUserEditedChannelDraft(false);

    setIsEditDialogOpen(false);
  }

  async function handleSaveChannelEdits(visibilityConfirmed = false) {
    // Public <-> private changes who can find and read the group: confirm.
    if (nextVisibility !== currentVisibility && !visibilityConfirmed) {
      setIsVisibilityConfirmOpen(true);
      return;
    }
    try {
      if (hasChannelEditChanges) {
        await updateChannelDetailsMutation.mutateAsync({
          description: descriptionDirty ? descriptionDraft.trim() : undefined,
          name: nameDirty ? nameDraft.trim() : undefined,
          picture: pictureDirty ? pictureDraft : undefined,
          ttlSeconds:
            nextTtlSeconds !== currentTtlSeconds ? nextTtlSeconds : undefined,
          visibility:
            lifecycleDirty && nextVisibility !== currentVisibility
              ? nextVisibility
              : undefined,
        });
      }

      setHasUserEditedChannelDraft(false);
      setIsEditDialogOpen(false);
    } catch {
      // React Query stores mutation errors; keep the dialog open and render them.
    }
  }

  return (
    <DialogPrimitive.Root
      modal={!isSplitLayout}
      onOpenChange={handlePanelOpenChange}
      open={open}
    >
      {!isSplitLayout ? (
        <DialogPrimitive.Portal>
          <DialogPrimitive.Overlay asChild>
            <OverlayPanelBackdrop
              onClose={() => handlePanelOpenChange(false)}
            />
          </DialogPrimitive.Overlay>
        </DialogPrimitive.Portal>
      ) : null}
      {isSplitLayout ? (
        // No translucent backdrop-blur surface here: `backdrop-filter`
        // creates a stacking context that traps the z-40 panel header below
        // the shared z-30 header blur strip in split layout. The pane sits on
        // the opaque `bg-background` from PANEL_BASE_CLASS instead.
        <DialogPrimitive.Content
          className={cn(
            PANEL_BASE_CLASS,
            "h-full w-full cursor-default overflow-hidden border-l-0 p-0",
            animateSplitEnter && PANEL_ENTER_MOTION_CLASS,
          )}
          data-testid="channel-management-sheet"
          onEscapeKeyDown={(event) => event.preventDefault()}
          onInteractOutside={(event) => event.preventDefault()}
          onPointerDownOutside={(event) => event.preventDefault()}
        >
          <ChannelManagementPanelContent
            activeView={visibleActiveView}
            archiveChannelMutation={archiveChannelMutation}
            canEditNarrative={canEditNarrative}
            canEditChannel={canEditChannel}
            canJoin={canJoin}
            canLeave={canLeave}
            canOpenCanvas={canOpenCanvas}
            canvasPreview={canvasPreview}
            canvasQuery={canvasQuery}
            channelId={channelId}
            currentPubkey={currentPubkey}
            isChat={isChat}
            workflowsEnabled={workflowsEnabled && !isChat}
            workflowsQuery={workflowsQuery}
            onCreateWorkflow={handleCreateWorkflow}
            onOpenWorkflow={handleOpenWorkflow}
            deleteChannelMutation={deleteChannelMutation}
            detailsError={detailsQuery.error}
            handleDeleteChannel={handleDeleteChannel}
            handleDeleteDialogOpenChange={handleDeleteDialogOpenChange}
            isArchived={isArchived}
            isDeleteDialogOpen={isDeleteDialogOpen}
            canDeleteChannel={canDeleteChannel}
            mode={auxiliaryPanelMode}
            transparentChrome={transparentChrome}
            joinChannelMutation={joinChannelMutation}
            leaveChannelMutation={leaveChannelMutation}
            memberCount={memberCount}
            members={members}
            membersError={membersQuery.error}
            onOpenEdit={openEditDialog}
            onOpenMembers={onOpenMembers ? openMembersDialog : undefined}
            onOpenChange={handlePanelOpenChange}
            resolvedChannel={resolvedChannel}
            setActiveView={setActiveView}
            goalsEnabled={goalsEnabled}
            canToggleGoals={goalsSwitch.canToggle}
            goalsSwitchError={goalsSwitch.error}
            isSavingGoals={goalsSwitch.isSaving}
            onGoalsEnabledChange={goalsSwitch.setEnabled}
            onOpenGoalLink={onOpenGoalLink}
            unarchiveChannelMutation={unarchiveChannelMutation}
          />
        </DialogPrimitive.Content>
      ) : (
        <DialogPrimitive.Portal>
          <DialogPrimitive.Content
            className={cn(
              PANEL_BASE_CLASS,
              PANEL_OVERLAY_CLASS,
              PANEL_ENTER_MOTION_CLASS,
              "w-[380px] cursor-default overflow-hidden p-0 data-[state=closed]:animate-out data-[state=closed]:slide-out-to-right data-[state=closed]:duration-200",
              isDark
                ? "bg-background/85 backdrop-blur-xl supports-backdrop-filter:bg-background/75"
                : "bg-background",
            )}
            data-testid="channel-management-sheet"
          >
            <ChannelManagementPanelContent
              activeView={visibleActiveView}
              archiveChannelMutation={archiveChannelMutation}
              canEditNarrative={canEditNarrative}
              canEditChannel={canEditChannel}
              canJoin={canJoin}
              canLeave={canLeave}
              canOpenCanvas={canOpenCanvas}
              canvasPreview={canvasPreview}
              canvasQuery={canvasQuery}
              channelId={channelId}
              currentPubkey={currentPubkey}
              isChat={isChat}
              workflowsEnabled={workflowsEnabled && !isChat}
              workflowsQuery={workflowsQuery}
              onCreateWorkflow={handleCreateWorkflow}
              onOpenWorkflow={handleOpenWorkflow}
              deleteChannelMutation={deleteChannelMutation}
              detailsError={detailsQuery.error}
              handleDeleteChannel={handleDeleteChannel}
              handleDeleteDialogOpenChange={handleDeleteDialogOpenChange}
              isArchived={isArchived}
              isDeleteDialogOpen={isDeleteDialogOpen}
              canDeleteChannel={canDeleteChannel}
              mode={auxiliaryPanelMode}
              transparentChrome={transparentChrome}
              joinChannelMutation={joinChannelMutation}
              leaveChannelMutation={leaveChannelMutation}
              memberCount={memberCount}
              members={members}
              membersError={membersQuery.error}
              onOpenEdit={openEditDialog}
              onOpenMembers={onOpenMembers ? openMembersDialog : undefined}
              onOpenChange={handlePanelOpenChange}
              resolvedChannel={resolvedChannel}
              setActiveView={setActiveView}
              goalsEnabled={goalsEnabled}
              canToggleGoals={goalsSwitch.canToggle}
              goalsSwitchError={goalsSwitch.error}
              isSavingGoals={goalsSwitch.isSaving}
              onGoalsEnabledChange={goalsSwitch.setEnabled}
              onOpenGoalLink={onOpenGoalLink}
              unarchiveChannelMutation={unarchiveChannelMutation}
            />
          </DialogPrimitive.Content>
        </DialogPrimitive.Portal>
      )}

      {canEditChannel ? (
        <Dialog
          onOpenChange={handleEditDialogOpenChange}
          open={isEditDialogOpen}
        >
          <DialogContent
            aria-describedby={undefined}
            className="max-w-lg overflow-hidden p-0"
          >
            <div className="flex max-h-[85vh] flex-col">
              <DialogHeader className="shrink-0 border-b border-border/60 px-6 py-5 pr-14">
                <DialogTitle>
                  {t("channels.edit.title", {
                    visibility:
                      nextVisibility === "private" ? "private" : "public",
                  })}
                </DialogTitle>
              </DialogHeader>

              <div className="min-h-0 flex-1 space-y-6 overflow-y-auto px-6 py-5">
                <div className="space-y-5">
                  <GroupPhotoField
                    channelId={resolvedChannel.id}
                    disabled={isSavingChannelEdits}
                    isPrivate={isPrivateDraft}
                    label={nameDraft || resolvedChannel.name}
                    onPictureChange={(picture) => {
                      setPictureDraft(picture);
                      setHasUserEditedChannelDraft(true);
                    }}
                    onUploadingChange={setIsPhotoUploading}
                    picture={pictureDraft}
                  />
                  <div className="space-y-1.5">
                    <label
                      className="text-sm font-medium text-foreground"
                      htmlFor="channel-name"
                    >
                      {t("channels.edit.name")}
                    </label>
                    <div
                      className={cn(
                        "flex min-h-11 items-center px-3",
                        CHANNEL_FORM_FIELD_SHELL_CLASS,
                      )}
                    >
                      <Input
                        className={cn(
                          "h-8 px-0 py-0 leading-6",
                          CHANNEL_FORM_FIELD_CONTROL_CLASS,
                        )}
                        data-testid="channel-management-name"
                        disabled={isSavingChannelEdits}
                        id="channel-name"
                        onChange={(event) => {
                          setNameDraft(event.target.value);
                          setHasUserEditedChannelDraft(true);
                        }}
                        value={nameDraft}
                      />
                    </div>
                  </div>
                  <div className="space-y-1.5">
                    <label
                      className="text-sm font-medium text-foreground"
                      htmlFor="channel-description"
                    >
                      {t("channels.edit.description")}
                    </label>
                    <div className={CHANNEL_FORM_FIELD_SHELL_CLASS}>
                      <Textarea
                        className={cn(
                          "min-h-20 resize-none px-3 py-3 leading-5",
                          CHANNEL_FORM_FIELD_CONTROL_CLASS,
                        )}
                        data-testid="channel-management-description"
                        disabled={isSavingChannelEdits}
                        id="channel-description"
                        onChange={(event) => {
                          setDescriptionDraft(event.target.value);
                          setHasUserEditedChannelDraft(true);
                        }}
                        rows={2}
                        value={descriptionDraft}
                      />
                    </div>
                  </div>
                </div>

                {resolvedChannel.channelType !== "dm" ? (
                  <div
                    className="space-y-5"
                    data-testid="channel-management-lifecycle"
                  >
                    <ChannelTypeSettings
                      channelId={resolvedChannel.id}
                      disabled={isSavingChannelEdits}
                      onTemporaryChange={(temporary) => {
                        setIsEphemeralDraft(temporary);
                        setHasUserEditedChannelDraft(true);
                      }}
                      onTtlSecondsChange={(ttlSeconds) => {
                        setTtlSecondsDraft(ttlSeconds);
                        setHasUserEditedChannelDraft(true);
                      }}
                      temporary={isEphemeralDraft}
                      testIdPrefix="channel-management"
                      ttlSeconds={ttlSecondsDraft}
                    />
                    <ChannelPermissionsSettings
                      disabled={isSavingChannelEdits}
                      onVisibilityChange={(visibility) => {
                        setIsPrivateDraft(visibility === "private");
                        setHasUserEditedChannelDraft(true);
                      }}
                      testIdPrefix="channel-management"
                      visibility={isPrivateDraft ? "private" : "open"}
                    />
                  </div>
                ) : null}

                {updateChannelDetailsMutation.error instanceof Error ? (
                  <p className="rounded-2xl border border-destructive/30 bg-destructive/10 px-4 py-3 text-sm text-destructive">
                    {updateChannelDetailsMutation.error.message}
                  </p>
                ) : null}
              </div>

              <div className="flex shrink-0 justify-end gap-2 border-t border-border/60 px-6 py-4">
                <Button
                  onClick={() => handleEditDialogOpenChange(false)}
                  size="sm"
                  type="button"
                  variant="outline"
                >
                  {t("common.cancel")}
                </Button>
                <Button
                  data-testid="channel-management-save-changes"
                  disabled={!canSaveChannelEdits}
                  onClick={() => void handleSaveChannelEdits(false)}
                  size="sm"
                  type="button"
                >
                  {isSavingChannelEdits
                    ? t("channels.edit.saving")
                    : t("channels.edit.save")}
                </Button>
              </div>
            </div>
          </DialogContent>
        </Dialog>
      ) : null}
      <GroupVisibilityConfirmDialog
        nextVisibility={nextVisibility}
        onConfirm={() => void handleSaveChannelEdits(true)}
        onOpenChange={setIsVisibilityConfirmOpen}
        open={isVisibilityConfirmOpen}
      />
    </DialogPrimitive.Root>
  );
}

type ChannelMutation<TArgs = void> = {
  error: unknown;
  isPending: boolean;
  mutateAsync: (args: TArgs) => Promise<unknown>;
};

type ChannelManagementPanelContentProps = {
  activeView: ManagementView;
  archiveChannelMutation: ChannelMutation;
  canEditChannel: boolean;
  canEditNarrative: boolean;
  canJoin: boolean;
  canLeave: boolean;
  canOpenCanvas: boolean;
  canvasPreview?: string;
  canvasQuery: { isLoading: boolean };
  channelId: string | null;
  currentPubkey?: string;
  goalsEnabled: boolean;
  canToggleGoals: boolean;
  goalsSwitchError: unknown;
  isSavingGoals: boolean;
  onGoalsEnabledChange: (enabled: boolean) => void;
  onOpenGoalLink?: (target: GoalLinkTarget) => void;
  /** Ownerless chat: show "Chat info" instead of group settings. */
  isChat: boolean;
  workflowsEnabled: boolean;
  workflowsQuery: {
    data?: Workflow[];
    error: unknown;
    isLoading: boolean;
    refetch: () => Promise<unknown>;
  };
  onCreateWorkflow: () => void;
  onOpenWorkflow: (workflow: Workflow) => void;
  deleteChannelMutation: ChannelMutation;
  detailsError: unknown;
  handleDeleteChannel: () => Promise<void>;
  handleDeleteDialogOpenChange: (open: boolean) => void;
  isArchived: boolean;
  isDeleteDialogOpen: boolean;
  canDeleteChannel: boolean; // true when caller may delete the channel
  mode: AuxiliaryPanelMode;
  transparentChrome?: boolean;
  joinChannelMutation: ChannelMutation;
  leaveChannelMutation: ChannelMutation<LeaveChannelArgs>;
  memberCount: number;
  members: ChannelMember[];
  membersError: unknown;
  onOpenEdit: () => void;
  onOpenMembers?: () => void;
  onOpenChange: (open: boolean) => void;
  resolvedChannel: Channel;
  setActiveView: React.Dispatch<React.SetStateAction<ManagementView>>;
  unarchiveChannelMutation: ChannelMutation;
};

function ChannelManagementPanelContent({
  activeView,
  archiveChannelMutation,
  canEditChannel,
  canEditNarrative,
  canJoin,
  canLeave,
  canOpenCanvas,
  canvasPreview,
  canvasQuery,
  channelId,
  currentPubkey,
  goalsEnabled,
  canToggleGoals,
  goalsSwitchError,
  isSavingGoals,
  onGoalsEnabledChange,
  onOpenGoalLink,
  isChat,
  workflowsEnabled,
  workflowsQuery,
  onCreateWorkflow,
  onOpenWorkflow,
  deleteChannelMutation,
  detailsError,
  handleDeleteChannel,
  handleDeleteDialogOpenChange,
  isArchived,
  isDeleteDialogOpen,
  canDeleteChannel,
  mode,
  transparentChrome = false,
  joinChannelMutation,
  leaveChannelMutation,
  memberCount,
  members,
  membersError,
  onOpenEdit,
  onOpenMembers,
  onOpenChange,
  resolvedChannel,
  setActiveView,
  unarchiveChannelMutation,
}: ChannelManagementPanelContentProps) {
  const t = useT();
  const scrollRef = React.useRef<HTMLDivElement>(null);
  useScrollBoundaryLock(scrollRef);
  const [isLeaveConfirmOpen, setIsLeaveConfirmOpen] = React.useState(false);
  const leave = (removeAgentPubkeys: readonly string[]) => {
    void leaveChannelMutation
      .mutateAsync({ removeAgentPubkeys })
      .then(() => onOpenChange(false));
  };

  return (
    <AuxiliaryPanelContext.Provider
      value={{
        isFloatingOverlay: mode === "panel",
        isOverlay: mode !== "docked",
        isSinglePanelView: mode === "single-panel",
        isSplitLayout: mode === "docked",
        layout: mode === "docked" ? "split" : "standalone",
        mode,
        onClose: () => onOpenChange(false),
        transparentChrome,
        widthPx: 380,
      }}
    >
      <AuxiliaryPanelHeader
        bordered={mode === "panel"}
        density={mode === "panel" ? "compact" : "comfortable"}
        mode={mode}
        transparent={transparentChrome}
      >
        <AuxiliaryPanelHeaderGroup
          backButtonAriaLabel={
            isChat
              ? t("channels.settings.backToChat")
              : t("channels.settings.backToGroup")
          }
          backButtonTestId="channel-management-back"
          mode={mode}
          onBack={
            activeView !== "summary"
              ? () => setActiveView("summary")
              : undefined
          }
        >
          <DialogPrimitive.Title asChild>
            <AuxiliaryPanelTitle>
              {activeView === "canvas"
                ? t("channels.settings.canvas")
                : activeView === "goals"
                  ? t("channels.goals.label")
                  : activeView === "workflows"
                    ? t("channels.settings.workflows")
                    : isChat
                      ? t("channels.settings.chatInfo")
                      : t("channels.settings.title")}
            </AuxiliaryPanelTitle>
          </DialogPrimitive.Title>
        </AuxiliaryPanelHeaderGroup>
        <DialogPrimitive.Description className="sr-only">
          {isChat
            ? t("channels.settings.chatInfo")
            : t("channels.settings.title")}
        </DialogPrimitive.Description>
      </AuxiliaryPanelHeader>

      <AuxiliaryPanelBody
        className="overflow-y-auto overflow-x-hidden overscroll-contain bg-background px-4 pb-8 [overflow-anchor:none]"
        mode={mode}
        panelPadding
        ref={scrollRef}
      >
        {activeView === "summary" && isChat ? (
          <ChatInfoSummary
            channel={resolvedChannel}
            currentPubkey={currentPubkey}
            goals={{
              canToggle: canToggleGoals,
              enabled: goalsEnabled,
              error: goalsSwitchError,
              isSaving: isSavingGoals,
              onChange: onGoalsEnabledChange,
              onOpenGoals: () => setActiveView("goals"),
            }}
            isLeaving={leaveChannelMutation.isPending}
            leaveError={leaveChannelMutation.error}
            memberCount={memberCount}
            members={members}
            membersError={membersError}
            onLeave={leave}
            onOpenPeople={onOpenMembers}
          />
        ) : activeView === "summary" ? (
          <div className="space-y-6 pt-3">
            <ChannelHero
              channel={resolvedChannel}
              onEdit={canEditChannel ? onOpenEdit : undefined}
            />

            {detailsError instanceof Error ? (
              <p className="rounded-xl border border-destructive/30 bg-destructive/10 px-3 py-2 text-sm text-destructive">
                {detailsError.message}
              </p>
            ) : null}

            {membersError instanceof Error ? (
              <p className="rounded-xl border border-destructive/30 bg-destructive/10 px-3 py-2 text-sm text-destructive">
                {membersError.message}
              </p>
            ) : null}

            <FieldGroup
              testId="channel-management-details"
              title={t("channels.settings.details")}
            >
              {resolvedChannel.channelType !== "dm" ? (
                <>
                  <ChannelTypeDetailRow
                    canEdit={canEditChannel}
                    channel={resolvedChannel}
                    onEdit={onOpenEdit}
                  />
                  <EditableInfoFieldRow
                    editTestId="channel-management-edit-visibility"
                    label={t("channels.settings.visibility")}
                    onEdit={canEditChannel ? onOpenEdit : undefined}
                    testId="channel-management-visibility"
                    value={
                      resolvedChannel.visibility === "private"
                        ? t("channels.settings.private")
                        : t("channels.settings.public")
                    }
                  />
                </>
              ) : null}
              <InfoFieldRow
                label={t("channels.settings.members")}
                onClick={onOpenMembers}
                testId="channel-management-member-count"
                trailing={
                  <ChannelMemberAvatarStack
                    currentPubkey={currentPubkey}
                    members={members}
                  />
                }
                value={t("channels.browser.memberCount", {
                  count: memberCount,
                })}
              />
              <CopyFieldRow
                icon={Fingerprint}
                label={t("channels.settings.groupId")}
                testId="channel-management-channel-id"
                value={resolvedChannel.id}
              />
            </FieldGroup>

            {resolvedChannel.channelType !== "dm" && !isArchived ? (
              <GroupSharingSettings
                canManage={canEditChannel}
                channelId={resolvedChannel.id}
                isMember={members.some(
                  (member) => member.pubkey === currentPubkey,
                )}
                visibility={resolvedChannel.visibility}
              />
            ) : null}

            <MuteNotificationsRow channelId={resolvedChannel.id} />

            <ChannelGoalsSwitchRow
              canToggle={canToggleGoals}
              checked={goalsEnabled}
              error={goalsSwitchError}
              isSaving={isSavingGoals}
              onCheckedChange={onGoalsEnabledChange}
            />
            <ChannelHistorySwitchRow
              canManageChannel={canEditChannel}
              channel={resolvedChannel}
            />

            {goalsEnabled ? (
              <IngressRow
                helpText={t("channels.settings.goalsHelp")}
                icon={Target}
                label={t("channels.goals.label")}
                onClick={() => setActiveView("goals")}
                testId="channel-goals-ingress"
              />
            ) : null}

            {canOpenCanvas ? (
              <div className="space-y-3">
                <IngressRow
                  description={canvasPreview}
                  helpText={t("channels.settings.canvasHelp")}
                  icon={BookOpenText}
                  label={t("channels.settings.canvas")}
                  onClick={() => setActiveView("canvas")}
                  testId="channel-canvas-ingress"
                  trailing={
                    canvasQuery.isLoading
                      ? t("channels.settings.loading")
                      : undefined
                  }
                />
                {workflowsEnabled ? (
                  <IngressRow
                    description={
                      workflowsQuery.isLoading
                        ? undefined
                        : t("channels.settings.workflowCount", {
                            count: workflowsQuery.data?.length ?? 0,
                          })
                    }
                    icon={WorkflowIcon}
                    label={t("channels.settings.workflows")}
                    onClick={() => setActiveView("workflows")}
                    testId="channel-workflows-ingress"
                    trailing={
                      workflowsQuery.isLoading
                        ? t("channels.settings.loading")
                        : undefined
                    }
                  />
                ) : null}
              </div>
            ) : workflowsEnabled ? (
              <IngressRow
                description={
                  workflowsQuery.isLoading
                    ? undefined
                    : t("channels.settings.workflowCount", {
                        count: workflowsQuery.data?.length ?? 0,
                      })
                }
                icon={WorkflowIcon}
                label={t("channels.settings.workflows")}
                onClick={() => setActiveView("workflows")}
                testId="channel-workflows-ingress"
                trailing={
                  workflowsQuery.isLoading
                    ? t("channels.settings.loading")
                    : undefined
                }
              />
            ) : null}

            {canJoin || canLeave || canEditChannel ? (
              <FieldGroup testId="channel-management-actions">
                {canJoin ? (
                  <ActionFieldRow
                    description={t("channels.settings.joinHint")}
                    disabled={joinChannelMutation.isPending}
                    icon={DoorOpen}
                    label={
                      joinChannelMutation.isPending
                        ? t("channels.settings.joining")
                        : t("channels.settings.join")
                    }
                    onClick={() => {
                      void joinChannelMutation.mutateAsync();
                    }}
                    testId="channel-management-join"
                  />
                ) : null}
                {canLeave ? (
                  <ActionFieldRow
                    disabled={leaveChannelMutation.isPending}
                    icon={DoorClosed}
                    label={
                      leaveChannelMutation.isPending
                        ? t("channels.settings.leaving")
                        : t("channels.settings.leave")
                    }
                    onClick={() => setIsLeaveConfirmOpen(true)}
                    testId="channel-management-leave"
                  />
                ) : null}
                {canLeave ? (
                  <LeaveChannelConfirmDialog
                    channelId={resolvedChannel.id}
                    channelName={resolvedChannel.name}
                    onLeave={leave}
                    onOpenChange={setIsLeaveConfirmOpen}
                    open={isLeaveConfirmOpen}
                  />
                ) : null}
                {canEditChannel ? (
                  isArchived ? (
                    <ActionFieldRow
                      disabled={unarchiveChannelMutation.isPending}
                      icon={ArchiveRestore}
                      label={
                        unarchiveChannelMutation.isPending
                          ? t("channels.settings.restoring")
                          : t("channels.archive.unarchive")
                      }
                      onClick={() => {
                        void unarchiveChannelMutation.mutateAsync();
                      }}
                      testId="channel-management-unarchive"
                    />
                  ) : (
                    <ActionFieldRow
                      disabled={archiveChannelMutation.isPending}
                      icon={Archive}
                      label={
                        archiveChannelMutation.isPending
                          ? t("channels.settings.archiving")
                          : t("channels.archive.archive")
                      }
                      onClick={() => {
                        void archiveChannelMutation.mutateAsync();
                      }}
                      testId="channel-management-archive"
                    />
                  )
                ) : null}
                {canEditChannel && canDeleteChannel ? (
                  <ChannelDeleteConfirmationDialog
                    channelName={resolvedChannel.name}
                    error={deleteChannelMutation.error}
                    isPending={deleteChannelMutation.isPending}
                    onConfirm={() => {
                      void handleDeleteChannel();
                    }}
                    onOpenChange={handleDeleteDialogOpenChange}
                    open={isDeleteDialogOpen}
                    trigger={
                      <ActionFieldRow
                        destructive
                        disabled={deleteChannelMutation.isPending}
                        icon={Trash2}
                        label={t("channels.delete.action")}
                        testId="channel-management-delete"
                      />
                    }
                  />
                ) : null}
              </FieldGroup>
            ) : null}

            {joinChannelMutation.error instanceof Error ? (
              <p className="text-sm text-destructive">
                {joinChannelMutation.error.message}
              </p>
            ) : null}
            {leaveChannelMutation.error instanceof Error ? (
              <p className="text-sm text-destructive">
                {leaveChannelMutation.error.message}
              </p>
            ) : null}
            {archiveChannelMutation.error instanceof Error ? (
              <p className="text-sm text-destructive">
                {archiveChannelMutation.error.message}
              </p>
            ) : null}
            {unarchiveChannelMutation.error instanceof Error ? (
              <p className="text-sm text-destructive">
                {unarchiveChannelMutation.error.message}
              </p>
            ) : null}
          </div>
        ) : activeView === "goals" && goalsEnabled && channelId ? (
          <GoalsPanel channelId={channelId} onOpenLink={onOpenGoalLink} />
        ) : activeView === "canvas" ? (
          <div data-testid="channel-canvas-section">
            <KeyedChannelCanvas
              canEdit={canEditNarrative}
              channelId={channelId}
              isArchived={isArchived}
            />
          </div>
        ) : activeView === "workflows" && workflowsEnabled ? (
          <ChannelWorkflowsSection
            error={workflowsQuery.error}
            loading={workflowsQuery.isLoading}
            onCreate={onCreateWorkflow}
            onOpen={onOpenWorkflow}
            onRetry={() => void workflowsQuery.refetch()}
            workflows={workflowsQuery.data ?? []}
          />
        ) : null}
      </AuxiliaryPanelBody>
    </AuxiliaryPanelContext.Provider>
  );
}
