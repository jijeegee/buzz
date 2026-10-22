import * as React from "react";
import {
  AlertCircle,
  BookUser,
  Check,
  ChevronRight,
  Download,
  Link2,
  X,
} from "lucide-react";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { toast } from "sonner";

import { useEncodeAgentSnapshotForSendMutation } from "@/features/agents/hooks";
import type { CatalogPersonaShareLevel } from "@/features/agents/lib/personaCatalogRelay";
import {
  useOpenDmMutation,
  useUpsertCachedChannel,
} from "@/features/channels/hooks";
import { buildSnapshotClipboardHtml } from "@/features/messages/lib/agentSnapshotClipboard";
import { uploadMediaBytes, type BlobDescriptor } from "@/shared/api/tauri";
import { copyTextToSystemClipboard } from "@/shared/api/tauriMedia";
import type { SnapshotMemoryLevel } from "@/shared/api/tauriPersonas";
import type { AgentPersona, UserSearchResult } from "@/shared/api/types";
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
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/shared/ui/dialog";
import { Separator } from "@/shared/ui/separator";
import { Spinner } from "@/shared/ui/spinner";
import { Switch } from "@/shared/ui/switch";

import {
  formatShareRecipientName,
  PersonaShareRecipients,
} from "./PersonaShareRecipients";
import { SnapshotOptionMenu } from "./SnapshotOptionMenu";
import { resolveSnapshotAvatarPng } from "./snapshotAvatarPng";
import { useSnapshotSendController } from "./useSnapshotSendController";
import { type Translate, useT } from "@/shared/i18n";

type PersonaShareDialogProps = {
  catalogShareLevel: CatalogPersonaShareLevel;
  isPending: boolean;
  linkedAgentPubkey: string | null;
  effectiveAvatarUrl: string | null;
  onCatalogShareLevelChange: (shareLevel: CatalogPersonaShareLevel) => void;
  onExport: () => void;
  onOpenChange: (open: boolean) => void;
  open: boolean;
  persona: AgentPersona;
};

type SnapshotShareDialogProps = {
  beforeExport?: React.ReactNode;
  displayName: string;
  encodeSnapshot: (
    memoryLevel: SnapshotMemoryLevel,
  ) => Promise<{ fileBytes: number[]; fileName: string }>;
  hasMemoryOptions: boolean;
  isPending: boolean;
  onExport: () => void;
  onOpenChange: (open: boolean) => void;
  onReset?: () => void;
  open: boolean;
  snapshotKind: "agent" | "team";
  testIdPrefix: string;
};

type EncodedSnapshot = {
  fileBytes: number[];
  fileName: string;
};

const RECIPIENT_ACTION_TRANSITION = {
  duration: 0.18,
  ease: [0.23, 1, 0.32, 1],
} as const;

const SHARE_WARNING_TRANSITION = {
  duration: 0.22,
  ease: [0.23, 1, 0.32, 1],
} as const;

const COPY_FEEDBACK_TRANSITION = {
  duration: 0.14,
  ease: [0.23, 1, 0.32, 1],
} as const;

const COPY_BUTTON_LAYOUT_TRANSITION = {
  duration: 0.2,
  ease: [0.23, 1, 0.32, 1],
} as const;

const COPY_FEEDBACK_RESET_MS = 1500;

type CopyStatus = "idle" | "copying" | "copied";

type PendingMemoryShare = {
  action: "copy" | "send";
  memoryLevel: Exclude<SnapshotMemoryLevel, "none">;
  recipientNames?: string[];
};

function buildSnapshotShareLevels(kind: "agent" | "team", t: Translate) {
  return [
    { value: "none" as const, label: t("agents.share.levelNone", { kind }) },
    {
      value: "core" as const,
      label: t("agents.share.levelCore", { kind }),
    },
    {
      value: "everything" as const,
      label: t("agents.share.levelAll", { kind }),
    },
  ];
}

function formatRecipientAudience(
  names: readonly string[],
  t: Translate,
): string {
  if (names.length === 0) return t("agents.share.audienceNone");
  if (names.length === 1) return names[0] ?? t("agents.share.audienceOne");
  if (names.length === 2)
    return t("agents.share.audienceTwo", {
      first: names[0] ?? "",
      second: names[1] ?? "",
    });
  return t("agents.share.audienceMany", {
    list: names.slice(0, -1).join(", "),
    last: names.at(-1) ?? "",
  });
}

function MemoryShareConfirmation({
  itemLabel,
  pendingShare,
  onCancel,
  onConfirm,
  testIdPrefix,
}: {
  itemLabel: string;
  pendingShare: PendingMemoryShare | null;
  onCancel: () => void;
  onConfirm: (pendingShare: PendingMemoryShare) => void;
  testIdPrefix: string;
}) {
  const t = useT();
  const isLinkShare = pendingShare?.action === "copy";
  const recipientAudience = formatRecipientAudience(
    pendingShare?.recipientNames ?? [],
    t,
  );

  return (
    <AlertDialog
      onOpenChange={(nextOpen) => {
        if (!nextOpen) onCancel();
      }}
      open={pendingShare !== null}
    >
      <AlertDialogContent data-testid={`${testIdPrefix}-memory-confirmation`}>
        <AlertDialogHeader>
          <AlertDialogTitle>{t("agents.share.memoriesTitle")}</AlertDialogTitle>
          <AlertDialogDescription>
            {t("agents.share.memoriesDescription", {
              kind: itemLabel,
              memory: pendingShare?.memoryLevel === "core" ? "core" : "all",
              link: isLinkShare ? "yes" : "no",
              audience: recipientAudience,
              b: (chunks) => <strong>{chunks}</strong>,
            })}
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel asChild>
            <Button type="button" variant="outline">
              {t("common.cancel")}
            </Button>
          </AlertDialogCancel>
          <AlertDialogAction asChild>
            <Button
              data-testid={`${testIdPrefix}-memory-confirm`}
              onClick={() => {
                if (pendingShare) onConfirm(pendingShare);
              }}
              type="button"
            >
              {isLinkShare ? t("agents.share.copyLink") : t("common.send")}
            </Button>
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}

function ShareLevelControl({
  ariaLabel,
  disabled,
  testId,
  value,
  options,
  onChange,
}: {
  ariaLabel: string;
  disabled: boolean;
  testId: string;
  value: SnapshotMemoryLevel;
  options: { value: SnapshotMemoryLevel; label: string }[];
  onChange: (level: SnapshotMemoryLevel) => void;
}) {
  return (
    <SnapshotOptionMenu
      ariaLabel={ariaLabel}
      disabled={disabled}
      onValueChange={(nextValue) => onChange(nextValue as SnapshotMemoryLevel)}
      options={options}
      testId={testId}
      value={value}
    />
  );
}

export function SnapshotShareDialog({
  beforeExport,
  displayName,
  encodeSnapshot,
  hasMemoryOptions,
  isPending,
  onExport,
  onOpenChange,
  onReset,
  open,
  snapshotKind,
  testIdPrefix,
}: SnapshotShareDialogProps) {
  const t = useT();
  const openDmMutation = useOpenDmMutation();
  const upsertCachedChannel = useUpsertCachedChannel();
  const snapshotSendController = useSnapshotSendController(open);
  const shouldReduceMotion = useReducedMotion();
  const [selectedRecipients, setSelectedRecipients] = React.useState<
    UserSearchResult[]
  >([]);
  const [copyStatus, setCopyStatus] = React.useState<CopyStatus>("idle");
  const [pendingMemoryShare, setPendingMemoryShare] =
    React.useState<PendingMemoryShare | null>(null);
  const [shareLevel, setShareLevel] =
    React.useState<SnapshotMemoryLevel>("none");
  const encodedSnapshotCacheRef = React.useRef(
    new Map<SnapshotMemoryLevel, Promise<EncodedSnapshot>>(),
  );

  const isSending = ["preparing", "uploading", "sending"].includes(
    snapshotSendController.state.phase,
  );
  const isCopying = copyStatus === "copying";
  const copyStatusLabel =
    copyStatus === "copying"
      ? t("agents.share.copying")
      : copyStatus === "copied"
        ? t("common.copied")
        : t("agents.share.copyLink");
  const isActionPending = isPending || isCopying || isSending;
  const isInterfacePending = isPending || isSending;
  const hasSelectedRecipients = selectedRecipients.length > 0;
  const showMemoryWarning = shareLevel !== "none";
  const recipientActionTransition = shouldReduceMotion
    ? { duration: 0 }
    : RECIPIENT_ACTION_TRANSITION;
  const warningTransition = shouldReduceMotion
    ? { duration: 0 }
    : SHARE_WARNING_TRANSITION;
  const copyFeedbackTransition = shouldReduceMotion
    ? { duration: 0 }
    : COPY_FEEDBACK_TRANSITION;
  const copyButtonLayoutTransition = shouldReduceMotion
    ? { duration: 0 }
    : COPY_BUTTON_LAYOUT_TRANSITION;
  const excludedRecipientPubkeys = React.useMemo(
    () =>
      snapshotSendController.relaySelfPubkey
        ? [snapshotSendController.relaySelfPubkey]
        : [],
    [snapshotSendController.relaySelfPubkey],
  );
  const itemLabel = snapshotKind === "team" ? "team" : "agent";
  const shareLevels = React.useMemo(
    () => buildSnapshotShareLevels(itemLabel, t),
    [itemLabel, t],
  );
  const getEncodedSnapshot = React.useCallback(
    (memoryLevel: SnapshotMemoryLevel) => {
      const effectiveMemoryLevel = hasMemoryOptions ? memoryLevel : "none";
      const cached = encodedSnapshotCacheRef.current.get(effectiveMemoryLevel);
      if (cached) return cached;

      const pending = encodeSnapshot(effectiveMemoryLevel).catch((error) => {
        if (
          encodedSnapshotCacheRef.current.get(effectiveMemoryLevel) === pending
        ) {
          encodedSnapshotCacheRef.current.delete(effectiveMemoryLevel);
        }
        throw error;
      });
      encodedSnapshotCacheRef.current.set(effectiveMemoryLevel, pending);
      return pending;
    },
    [encodeSnapshot, hasMemoryOptions],
  );

  React.useEffect(() => {
    if (open) {
      encodedSnapshotCacheRef.current.clear();
      setSelectedRecipients([]);
      setCopyStatus("idle");
      setPendingMemoryShare(null);
      setShareLevel("none");
      onReset?.();
      snapshotSendController.reset();
    }
  }, [open, onReset, snapshotSendController.reset]);

  React.useEffect(() => {
    if (copyStatus !== "copied") return;

    const resetTimer = window.setTimeout(
      () => setCopyStatus("idle"),
      COPY_FEEDBACK_RESET_MS,
    );
    return () => window.clearTimeout(resetTimer);
  }, [copyStatus]);

  async function uploadSnapshot(
    memoryLevel: SnapshotMemoryLevel,
  ): Promise<BlobDescriptor> {
    const encoded = await getEncodedSnapshot(memoryLevel);
    const uploaded = await uploadMediaBytes(
      encoded.fileBytes,
      encoded.fileName,
    );
    const { thumb: _thumb, ...uploadedWithoutThumb } = uploaded;

    return {
      ...uploadedWithoutThumb,
      filename: encoded.fileName,
    };
  }

  async function copyLink(memoryLevel: SnapshotMemoryLevel) {
    if (isActionPending) return;

    setCopyStatus("copying");
    try {
      const uploaded = await uploadSnapshot(memoryLevel);
      await copyTextToSystemClipboard(
        uploaded.url,
        buildSnapshotClipboardHtml({
          attachment: uploaded,
          displayName,
          snapshotKind,
        }),
      );
      setCopyStatus("copied");
    } catch {
      setCopyStatus("idle");
      toast.error(t("agents.share.copyFailed"));
    }
  }

  async function sendToRecipients(memoryLevel: SnapshotMemoryLevel) {
    if (isActionPending || selectedRecipients.length === 0) return;

    // One 1:1 per recipient: sharing never starts a multi-person chat.
    let sent: boolean | null = true;
    for (const recipient of selectedRecipients) {
      sent = await snapshotSendController.beginSend(
        () => getEncodedSnapshot(memoryLevel),
        async () => {
          const directMessage = await openDmMutation.mutateAsync({
            pubkeys: [recipient.pubkey],
          });
          await upsertCachedChannel(directMessage);
          return directMessage.id;
        },
        displayName,
      );
      if (sent !== true) break;
    }

    if (sent) {
      toast.success(t("agents.share.sent", { name: displayName }));
      onOpenChange(false);
    } else if (sent === false) {
      toast.error(
        snapshotSendController.getCurrentError() ??
          t("agents.share.sendFailed", { kind: itemLabel }),
      );
    }
  }

  function requestMemoryShare(
    action: PendingMemoryShare["action"],
    memoryLevel: SnapshotMemoryLevel,
  ) {
    if (isActionPending) return;

    const effectiveMemoryLevel = hasMemoryOptions ? memoryLevel : "none";
    if (effectiveMemoryLevel !== "none") {
      setPendingMemoryShare({
        action,
        memoryLevel: effectiveMemoryLevel,
        recipientNames:
          action === "send"
            ? selectedRecipients.map(formatShareRecipientName)
            : undefined,
      });
      return;
    }

    if (action === "copy") {
      void copyLink("none");
    } else {
      void sendToRecipients("none");
    }
  }

  function confirmMemoryShare(pendingShare: PendingMemoryShare) {
    setPendingMemoryShare(null);
    if (pendingShare.action === "copy") {
      void copyLink(pendingShare.memoryLevel);
    } else {
      void sendToRecipients(pendingShare.memoryLevel);
    }
  }

  function handleDialogOpenChange(nextOpen: boolean) {
    if (!nextOpen && isActionPending) return;
    onOpenChange(nextOpen);
  }

  return (
    <Dialog onOpenChange={handleDialogOpenChange} open={open}>
      <DialogContent
        className="max-w-xl gap-3 bg-transparent p-0 shadow-none"
        data-testid={`${testIdPrefix}-dialog`}
        showCloseButton={false}
      >
        <div
          className="relative rounded-2xl bg-background p-6 pb-4 shadow-2xl"
          data-testid={`${testIdPrefix}-main-card`}
        >
          <DialogHeader>
            <DialogTitle className="min-w-0 truncate pr-10">
              {t("agents.share.title", { name: displayName })}
            </DialogTitle>
            <DialogDescription
              data-testid={`${testIdPrefix}-share-description`}
            >
              {t("agents.share.description", { kind: itemLabel })}
            </DialogDescription>
          </DialogHeader>
          <DialogClose
            className="absolute right-4 top-4 flex h-8 w-8 shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors duration-150 ease-out hover:bg-accent hover:text-accent-foreground focus:outline-hidden focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-100"
            disabled={isActionPending}
          >
            <X className="h-4 w-4" />
            <span className="sr-only">{t("common.close")}</span>
          </DialogClose>

          <div className="space-y-4 pt-4">
            <div className="space-y-2">
              <div className="flex items-start gap-2">
                <motion.div
                  className="min-w-0 flex-1"
                  layout
                  transition={recipientActionTransition}
                >
                  <PersonaShareRecipients
                    disabled={
                      isInterfacePending ||
                      !snapshotSendController.isDmSafetyReady
                    }
                    excludedPubkeys={excludedRecipientPubkeys}
                    onSelectionChange={setSelectedRecipients}
                    open={open}
                    selectedUsers={selectedRecipients}
                    testIdPrefix={testIdPrefix}
                  />
                </motion.div>
                <AnimatePresence initial={false} mode="popLayout">
                  {hasSelectedRecipients ? (
                    <motion.div
                      animate={{ opacity: 1 }}
                      className="shrink-0"
                      data-testid={`${testIdPrefix}-send-motion`}
                      exit={{ opacity: 0 }}
                      initial={shouldReduceMotion ? false : { opacity: 0 }}
                      layout
                      transition={recipientActionTransition}
                    >
                      <Button
                        className="h-10"
                        data-testid={`${testIdPrefix}-send`}
                        disabled={
                          isActionPending ||
                          !snapshotSendController.isDmSafetyReady
                        }
                        onClick={() => requestMemoryShare("send", shareLevel)}
                        type="button"
                      >
                        {isSending
                          ? t("agents.share.sending")
                          : t("common.send")}
                      </Button>
                    </motion.div>
                  ) : null}
                </AnimatePresence>
              </div>
            </div>

            {hasMemoryOptions ? (
              <section
                className="space-y-2"
                data-testid={`${testIdPrefix}-link-settings`}
              >
                <h3 className="text-xs font-medium text-secondary-foreground/75">
                  {t("agents.share.settings")}
                </h3>
                <div
                  className="flex items-center gap-3"
                  data-testid={`${testIdPrefix}-share-level-row`}
                >
                  <h4 className="min-w-0 flex-1 text-sm font-medium">
                    {t("agents.share.included")}
                  </h4>
                  <ShareLevelControl
                    ariaLabel={t("agents.share.whatToInclude")}
                    disabled={isInterfacePending}
                    onChange={setShareLevel}
                    options={shareLevels}
                    testId={`${testIdPrefix}-share-level`}
                    value={shareLevel}
                  />
                </div>
              </section>
            ) : null}

            <Separator
              className="bg-input/40"
              data-testid={`${testIdPrefix}-link-divider`}
            />

            <div
              className="flex justify-end"
              data-testid={`${testIdPrefix}-link-row`}
            >
              <Button
                asChild
                className="shrink-0 border-border shadow-none disabled:opacity-100"
                data-copy-status={copyStatus}
                data-testid={`${testIdPrefix}-copy-link`}
                disabled={isActionPending}
                onClick={() => requestMemoryShare("copy", shareLevel)}
                size="sm"
                type="button"
                variant="outline"
              >
                <motion.button
                  layout={!shouldReduceMotion}
                  layoutDependency={copyStatus}
                  style={{ transformOrigin: "100% 50%" }}
                  transition={copyButtonLayoutTransition}
                >
                  <span aria-live="polite" className="sr-only">
                    {copyStatusLabel}
                  </span>
                  <span
                    aria-hidden="true"
                    className="relative grid place-items-center"
                    data-testid={`${testIdPrefix}-copy-link-stage`}
                  >
                    <AnimatePresence initial={false} mode="popLayout">
                      <motion.span
                        animate={{
                          filter: "blur(0px)",
                          opacity: 1,
                          transform: "scale(1)",
                        }}
                        className="col-start-1 row-start-1 flex items-center justify-center gap-1.5 [transform-origin:50%_50%] will-change-[transform,opacity,filter]"
                        data-testid={`${testIdPrefix}-copy-link-state`}
                        exit={
                          shouldReduceMotion
                            ? { opacity: 0 }
                            : {
                                filter: "blur(2px)",
                                opacity: 0,
                                transform: "scale(0.97)",
                              }
                        }
                        initial={
                          shouldReduceMotion
                            ? false
                            : {
                                filter: "blur(2px)",
                                opacity: 0,
                                transform: "scale(0.97)",
                              }
                        }
                        key={copyStatus}
                        transition={copyFeedbackTransition}
                      >
                        {copyStatus === "copying" ? (
                          <Spinner
                            aria-hidden="true"
                            className="h-4 w-4 border-2"
                          />
                        ) : copyStatus === "copied" ? (
                          <Check aria-hidden="true" className="h-4 w-4" />
                        ) : (
                          <Link2 aria-hidden="true" className="h-4 w-4" />
                        )}
                        <span>{copyStatusLabel}</span>
                      </motion.span>
                    </AnimatePresence>
                  </span>
                </motion.button>
              </Button>
            </div>

            <AnimatePresence initial={false}>
              {showMemoryWarning ? (
                <motion.div
                  animate={{ height: "auto", opacity: 1 }}
                  className="overflow-hidden"
                  data-testid={`${testIdPrefix}-memory-warning-motion`}
                  exit={{ height: 0, opacity: 0 }}
                  initial={{ height: 0, opacity: 0 }}
                  key={`${testIdPrefix}-memory-warning`}
                  transition={warningTransition}
                >
                  <div
                    className="flex items-start gap-2 rounded-md border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-sm text-amber-700 dark:text-amber-400"
                    data-testid={`${testIdPrefix}-memory-warning`}
                  >
                    <AlertCircle className="mt-0.5 h-4 w-4 shrink-0" />
                    <p>
                      {t("agents.export.memoryWarning", {
                        b: (chunks) => <strong>{chunks}</strong>,
                      })}
                    </p>
                  </div>
                </motion.div>
              ) : null}
            </AnimatePresence>
          </div>
        </div>
        {beforeExport}
        <button
          className="relative flex min-h-14 w-full items-center gap-3 rounded-2xl bg-background px-5 py-4 text-left text-sm font-medium shadow-2xl outline-hidden transition-colors hover:bg-muted focus-visible:bg-muted disabled:cursor-default disabled:opacity-100"
          data-testid={`${testIdPrefix}-export`}
          disabled={isActionPending}
          onClick={onExport}
          type="button"
        >
          <Download className="h-4 w-4 shrink-0 text-muted-foreground" />
          <span className="min-w-0 flex-1">
            {t("agents.share.export", { kind: itemLabel })}
          </span>
          <ChevronRight className="h-4 w-4 shrink-0 text-muted-foreground" />
        </button>
      </DialogContent>
      <MemoryShareConfirmation
        itemLabel={itemLabel}
        onCancel={() => setPendingMemoryShare(null)}
        onConfirm={confirmMemoryShare}
        pendingShare={pendingMemoryShare}
        testIdPrefix={testIdPrefix}
      />
    </Dialog>
  );
}

export function PersonaShareDialog({
  catalogShareLevel,
  isPending,
  linkedAgentPubkey,
  effectiveAvatarUrl,
  onCatalogShareLevelChange,
  onExport,
  onOpenChange,
  open,
  persona,
}: PersonaShareDialogProps) {
  const t = useT();
  const encodeSnapshotMutation = useEncodeAgentSnapshotForSendMutation();
  const encodeSnapshot = React.useCallback(
    async (memoryLevel: SnapshotMemoryLevel) =>
      encodeSnapshotMutation.mutateAsync({
        id: persona.id,
        memoryLevel: linkedAgentPubkey ? memoryLevel : "none",
        format: "png",
        memorySourcePubkey: linkedAgentPubkey,
        avatarPngDataUrl: await resolveSnapshotAvatarPng(effectiveAvatarUrl),
      }),
    [
      encodeSnapshotMutation.mutateAsync,
      effectiveAvatarUrl,
      linkedAgentPubkey,
      persona.id,
    ],
  );

  return (
    <SnapshotShareDialog
      beforeExport={
        persona.isBuiltIn ? null : (
          <section
            className="relative flex min-h-16 w-full items-center gap-3 rounded-2xl bg-background px-5 py-4 shadow-2xl"
            data-testid="persona-share-catalog"
          >
            <BookUser className="h-4 w-4 shrink-0 text-muted-foreground" />
            <div className="min-w-0 flex-1">
              <h3 className="text-sm font-medium">
                {t("agents.share.toCatalog")}
              </h3>
              <p className="text-xs text-secondary-foreground/75">
                {t("agents.share.toCatalogDescription")}
              </p>
            </div>
            <Switch
              aria-label={t("agents.share.toCatalog")}
              checked={catalogShareLevel !== "not-shared"}
              data-testid="persona-share-catalog-access"
              disabled={isPending}
              onCheckedChange={(checked) =>
                onCatalogShareLevelChange(checked ? "none" : "not-shared")
              }
              style={{ cursor: "default" }}
            />
          </section>
        )
      }
      displayName={persona.displayName}
      encodeSnapshot={encodeSnapshot}
      hasMemoryOptions={linkedAgentPubkey !== null}
      isPending={isPending}
      onExport={onExport}
      onOpenChange={onOpenChange}
      onReset={encodeSnapshotMutation.reset}
      open={open}
      snapshotKind="agent"
      testIdPrefix="persona-share"
    />
  );
}
