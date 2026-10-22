import * as React from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { ExternalLink } from "lucide-react";

import {
  useChatGptPlanSignIn,
  useChatGptPlanSignOut,
  useChatGptPlanStatus,
} from "@/features/agents/useChatGptPlan";
import { CHATGPT_MANAGE_USAGE_URL } from "@/shared/api/tauriChatGptPlan";
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
import { useT, t } from "@/shared/i18n";

function errorText(error: unknown): string {
  return typeof error === "string" ? error : t("agents.chatgpt.signInFailed");
}

export function ManageUsageLink({ className }: { className?: string }) {
  const t = useT();
  return (
    <button
      className={cn(
        "inline-flex shrink-0 items-center gap-1 text-xs font-medium text-primary hover:underline",
        className,
      )}
      onClick={() => void openUrl(CHATGPT_MANAGE_USAGE_URL)}
      type="button"
    >
      {t("agents.chatgpt.manageUsage")}
      <ExternalLink aria-hidden="true" className="h-3 w-3" />
    </button>
  );
}

/**
 * Starts Kuzz's own "Sign in with ChatGPT" in the system browser. After the
 * first sign-in on this computer it shows OpenAI's one-time plan notice.
 */
export function ContinueWithChatGptButton({
  className,
}: {
  className?: string;
}) {
  const t = useT();
  const signIn = useChatGptPlanSignIn();
  const [showNotice, setShowNotice] = React.useState(false);

  async function handleClick() {
    try {
      const result = await signIn.mutateAsync();
      if (result.firstSignIn && result.status.planEnabled) setShowNotice(true);
    } catch {
      // Shown below from the mutation state.
    }
  }

  return (
    <div className={cn("flex flex-col items-end gap-1", className)}>
      <Button
        className="bg-black text-white hover:bg-black/85 dark:bg-white dark:text-black dark:hover:bg-white/85"
        data-testid="chatgpt-plan-sign-in"
        disabled={signIn.isPending}
        onClick={() => void handleClick()}
        size="sm"
        type="button"
      >
        {signIn.isPending ? (
          <>
            <Spinner aria-hidden className="h-3.5 w-3.5 border-2" />
            {t("agents.chatgpt.finishInBrowser")}
          </>
        ) : (
          t("agents.chatgpt.continue")
        )}
      </Button>
      {signIn.isError ? (
        <p className="text-xs text-destructive" role="alert">
          {errorText(signIn.error)}
        </p>
      ) : null}
      <Dialog onOpenChange={setShowNotice} open={showNotice}>
        <DialogContent
          className="max-w-sm text-center"
          data-testid="chatgpt-plan-first-notice"
        >
          <DialogHeader className="items-center text-center">
            <DialogTitle>{t("agents.chatgpt.noticeTitle")}</DialogTitle>
            <DialogDescription className="text-center">
              {t("agents.chatgpt.noticeDescription")}
            </DialogDescription>
          </DialogHeader>
          <DialogFooter className="flex-col items-center gap-3 sm:flex-col sm:justify-center">
            <Button onClick={() => setShowNotice(false)} type="button">
              {t("agents.chatgpt.gotIt")}
            </Button>
            <ManageUsageLink />
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}

/** Settings › Models › Providers › OpenAI: the ChatGPT plan connection. */
export function ChatGptPlanSettingsRow() {
  const t = useT();
  const status = useChatGptPlanStatus();
  const signOut = useChatGptPlanSignOut();
  const data = status.data;
  const connected = Boolean(data?.signedIn && data.planEnabled);

  let detail: React.ReactNode = t("agents.chatgpt.detail");
  if (data?.signedIn && !data.planEnabled) {
    detail = t("agents.chatgpt.notAllowed");
  } else if (connected) {
    detail = data?.email
      ? t("agents.chatgpt.connectedAs", { email: data.email })
      : t("agents.chatgpt.connected");
  }
  if (signOut.isSuccess && signOut.data === false && !data?.signedIn) {
    detail = t("agents.chatgpt.signedOutUnconfirmed");
  }

  return (
    <div
      className="flex min-h-16 items-center gap-3 px-4 py-3 text-sm"
      data-testid="settings-models-openai-chatgpt-plan"
    >
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-center gap-2">
          <p className="text-sm font-medium">
            {connected
              ? t("agents.chatgpt.planTitle")
              : t("agents.chatgpt.usePlanTitle")}
          </p>
          {connected ? (
            <span className="inline-flex shrink-0 items-center rounded-md bg-emerald-500/15 px-2 py-0.5 text-xs font-medium text-emerald-600 dark:text-emerald-400">
              {t("agents.chatgpt.connectedBadge")}
            </span>
          ) : null}
        </div>
        <p
          className="mt-0.5 text-sm text-muted-foreground/70"
          data-settings-subcopy
        >
          {detail}
        </p>
        {signOut.isError ? (
          <p className="mt-1 text-xs text-destructive" role="alert">
            {errorText(signOut.error)}
          </p>
        ) : null}
      </div>
      {status.isPending ? (
        <Spinner aria-hidden className="h-4 w-4 border-2" />
      ) : connected ? (
        <div className="flex shrink-0 items-center gap-3">
          <ManageUsageLink />
          <Button
            data-testid="chatgpt-plan-sign-out"
            disabled={signOut.isPending}
            onClick={() => signOut.mutate()}
            size="sm"
            type="button"
            variant="outline"
          >
            {signOut.isPending ? (
              <Spinner aria-hidden className="h-3.5 w-3.5 border-2" />
            ) : (
              t("agents.chatgpt.signOut")
            )}
          </Button>
        </div>
      ) : (
        <ContinueWithChatGptButton />
      )}
    </div>
  );
}

/**
 * Agent settings, under the model picker, when the provider is the ChatGPT
 * plan: sign in in place, or show that requests use the plan.
 */
export function ChatGptPlanAgentNotice() {
  const t = useT();
  const status = useChatGptPlanStatus();
  if (status.isPending) return null;
  const connected = Boolean(status.data?.signedIn && status.data.planEnabled);
  if (connected) {
    return (
      <div
        className="flex items-center gap-2 rounded-lg bg-muted/50 px-3 py-2 text-xs"
        data-testid="chatgpt-plan-in-use"
      >
        <span className="flex-1 text-muted-foreground">
          {t("agents.chatgpt.using")}
        </span>
        <ManageUsageLink />
      </div>
    );
  }
  return (
    <div
      className="flex items-center gap-3 rounded-lg border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-xs text-amber-700 dark:text-amber-400"
      data-testid="chatgpt-plan-sign-in-needed"
    >
      <span className="flex-1">{t("agents.chatgpt.connectToUse")}</span>
      <ContinueWithChatGptButton />
    </div>
  );
}
