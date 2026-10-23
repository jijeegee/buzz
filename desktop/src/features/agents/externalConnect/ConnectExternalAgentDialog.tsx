import * as React from "react";
import { CircleCheck, Clock, Copy, TriangleAlert } from "lucide-react";
import { toast } from "sonner";
import { useQueryClient } from "@tanstack/react-query";

import type { AgentPairing } from "@/shared/api/agentPairing";
import { writeTextToClipboard } from "@/shared/lib/clipboard";
import { useT } from "@/shared/i18n";
import { cn } from "@/shared/lib/cn";
import { Button } from "@/shared/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/shared/ui/dialog";
import { Spinner } from "@/shared/ui/spinner";
import { formatCountdown } from "./pairingLogic";
import {
  formatConnectedAt,
  refreshExternalAgents,
  rememberConnection,
  useExternalAgentName,
} from "./useExternalAgents";
import { usePairingSession } from "./usePairingSession";

/**
 * "Connect external agent": one `kuzz pair --relay <url> <code>` command to paste on the
 * other computer, its ten-minute countdown, and the connected notice once
 * the first claim arrives (no approve step).
 */
export function ConnectExternalAgentDialog({
  open,
  owner,
  onOpenChange,
}: {
  open: boolean;
  /** The signed-in account, for remembering connections. */
  owner: string;
  onOpenChange: (open: boolean) => void;
}) {
  const t = useT();
  const queryClient = useQueryClient();
  const onConnected = React.useCallback(
    (claim: AgentPairing) => {
      rememberConnection(owner, claim);
      refreshExternalAgents(queryClient);
    },
    [owner, queryClient],
  );
  const { phase, secondsLeft, newCode } = usePairingSession({
    open,
    failedText: t("agents.external.startFailed"),
    onConnected,
  });

  const command =
    phase.kind === "starting" || phase.kind === "error"
      ? null
      : phase.start.command;
  const used = phase.kind === "connected" || phase.kind === "expired";

  return (
    <Dialog onOpenChange={onOpenChange} open={open}>
      <DialogContent
        className="max-w-xl gap-0 p-0"
        data-testid="connect-external-agent-dialog"
      >
        <DialogHeader className="border-b border-border/70 px-6 pb-4 pt-6">
          <DialogTitle>{t("agents.external.title")}</DialogTitle>
          <DialogDescription>
            {t("agents.external.description")}
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-4 px-6 py-5">
          {phase.kind === "connected" ? (
            <ConnectedNotice claim={phase.claim} />
          ) : null}
          {phase.kind === "expired" ? (
            <Notice tone="warning">{t("agents.external.expired")}</Notice>
          ) : null}
          {phase.kind === "error" ? (
            <Notice tone="error">{phase.message}</Notice>
          ) : null}

          {phase.kind === "starting" ? (
            <p className="flex items-center gap-2 text-sm text-muted-foreground">
              <Spinner className="h-4 w-4" />
              {t("agents.external.making")}
            </p>
          ) : null}

          {command ? (
            <div className="space-y-2">
              <p
                className={cn(
                  "text-sm font-medium",
                  used && "text-muted-foreground",
                )}
              >
                {used
                  ? t("agents.external.usedLabel")
                  : t("agents.external.pasteLabel")}
              </p>
              <CommandBox command={command} key={command} used={used} />
              {phase.kind === "waiting" ? (
                <p
                  className="flex items-center gap-1.5 text-xs text-muted-foreground"
                  data-testid="connect-external-agent-countdown"
                >
                  <Clock aria-hidden="true" className="h-3.5 w-3.5" />
                  <span className="tabular-nums">
                    {t("agents.external.expiresIn", {
                      time: formatCountdown(secondsLeft),
                    })}
                  </span>
                </p>
              ) : null}
              {phase.kind === "connected" ? (
                <p className="text-xs text-muted-foreground">
                  {t("agents.external.used")}
                </p>
              ) : null}
            </div>
          ) : null}

          {phase.kind === "waiting" ? (
            <p
              aria-live="polite"
              className="flex items-center gap-2 rounded-lg bg-muted/50 px-4 py-3 text-sm text-muted-foreground"
            >
              <Spinner className="h-4 w-4" />
              {t("agents.external.waiting")}
            </p>
          ) : null}
        </div>

        <DialogFooter className="border-t border-border/70 px-6 py-4">
          {phase.kind === "starting" ? null : (
            <Button
              data-testid="connect-external-agent-new-code"
              onClick={newCode}
              type="button"
              variant="outline"
            >
              {phase.kind === "error"
                ? t("agents.external.tryAgain")
                : t("agents.external.newCode")}
            </Button>
          )}
          <Button
            onClick={() => onOpenChange(false)}
            type="button"
            variant={phase.kind === "connected" ? "default" : "outline"}
          >
            {phase.kind === "connected" ? t("common.done") : t("common.cancel")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function CommandBox({ command, used }: { command: string; used: boolean }) {
  const t = useT();
  const [copied, setCopied] = React.useState(false);
  React.useEffect(() => {
    if (!copied) return;
    const timer = window.setTimeout(() => setCopied(false), 2_000);
    return () => window.clearTimeout(timer);
  }, [copied]);

  return (
    <div
      className={cn(
        "flex min-w-0 items-center gap-2 rounded-lg border border-border/80 py-1.5 pl-3 pr-1.5",
        used ? "bg-muted/40" : "bg-muted/20",
      )}
    >
      <code
        className={cn(
          "min-w-0 flex-1 select-all break-all font-mono text-sm",
          used && "text-muted-foreground line-through",
        )}
        data-testid="connect-external-agent-command"
      >
        {command}
      </code>
      {used ? null : (
        <Button
          aria-label={t("agents.external.copyLabel")}
          data-testid="connect-external-agent-copy"
          onClick={() => {
            void writeTextToClipboard(command)
              .then(() => setCopied(true))
              .catch(() => toast.error(t("ui.clipboard.copyFailed")));
          }}
          size="sm"
          type="button"
          variant="outline"
        >
          <Copy aria-hidden="true" />
          {copied ? t("common.copied") : t("common.copy")}
        </Button>
      )}
    </div>
  );
}

function Notice({
  tone,
  children,
}: {
  tone: "warning" | "error";
  children: React.ReactNode;
}) {
  return (
    <p
      className={cn(
        "flex items-start gap-2 rounded-lg border px-4 py-3 text-sm",
        tone === "error"
          ? "border-destructive/30 bg-destructive/10 text-destructive"
          : "border-amber-500/30 bg-amber-500/10 text-amber-800 dark:text-amber-300",
      )}
      role="status"
    >
      <TriangleAlert aria-hidden="true" className="mt-0.5 h-4 w-4 shrink-0" />
      <span>{children}</span>
    </p>
  );
}

function ConnectedNotice({ claim }: { claim: AgentPairing }) {
  const t = useT();
  const agent = useExternalAgentName(claim.agentPubkey);
  const device = claim.deviceName?.trim() || t("agents.external.otherComputer");
  const at = formatConnectedAt(claim.claimedAt);
  return (
    <div
      className="flex items-center gap-3 rounded-lg border border-emerald-500/30 bg-emerald-500/10 px-4 py-3"
      data-testid="connect-external-agent-connected"
      role="status"
    >
      <CircleCheck
        aria-hidden="true"
        className="h-5 w-5 shrink-0 text-emerald-600 dark:text-emerald-400"
      />
      <div className="min-w-0 text-sm">
        <p className="font-medium">
          {t("agents.external.connected", { agent, device })}
        </p>
        {at ? (
          <p className="text-xs text-muted-foreground">
            {t("agents.external.connectedAt", { time: at })}
          </p>
        ) : null}
      </div>
    </div>
  );
}
