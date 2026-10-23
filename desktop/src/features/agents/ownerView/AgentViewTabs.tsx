import * as React from "react";
import { toast } from "sonner";

import { useAppNavigation } from "@/app/navigation/useAppNavigation";
import { useOpenDmMutation } from "@/features/channels/hooks";
import { useUserProfileQuery } from "@/features/profile/hooks";
import { cn } from "@/shared/lib/cn";
import { useT } from "@/shared/i18n";

import { useAgentChatList, useOwnerViewBinding } from "./useAgentChatList";

/** The signed-in account owns this agent (the relay's owner record). */
export function useIsMyAgent(agentPubkey: string | null | undefined) {
  const binding = useOwnerViewBinding();
  const ownerPubkey = useUserProfileQuery(agentPubkey ?? undefined).data
    ?.ownerPubkey;
  return (
    !!binding &&
    !!ownerPubkey &&
    ownerPubkey.toLowerCase() === binding.ownerPubkey
  );
}

/**
 * `Chat with Fizz | Fizz's conversations`: the DM with my agent, or the
 * chats it has without me. Renders nothing for someone else's agent.
 */
export function AgentViewTabs({
  active,
  agentName,
  agentPubkey,
}: {
  active: "chat" | "conversations";
  agentName: string;
  agentPubkey: string;
}) {
  const isMine = useIsMyAgent(agentPubkey);
  if (!isMine) return null;
  return (
    <AgentViewTabsInner
      active={active}
      agentName={agentName}
      agentPubkey={agentPubkey}
    />
  );
}

function AgentViewTabsInner({
  active,
  agentName,
  agentPubkey,
}: {
  active: "chat" | "conversations";
  agentName: string;
  agentPubkey: string;
}) {
  const t = useT();
  const { goChannel } = useAppNavigation();
  const openDmMutation = useOpenDmMutation();
  const { rows } = useAgentChatList(agentPubkey);
  const dots = React.useMemo(
    () => rows?.filter((row) => row.unread_mark).length ?? 0,
    [rows],
  );
  const openDm = async (viewAs?: string) => {
    try {
      const dm = await openDmMutation.mutateAsync({ pubkeys: [agentPubkey] });
      await goChannel(dm.id, viewAs ? { viewAs } : undefined);
    } catch (error) {
      toast.error(
        error instanceof Error ? error.message : t("agents.list.openDmFailed"),
      );
    }
  };
  const tabClass = (selected: boolean) =>
    cn(
      "rounded-md px-3 py-1 text-sm transition-colors",
      selected
        ? "bg-background font-medium text-foreground shadow-xs"
        : "text-muted-foreground hover:text-foreground",
    );
  return (
    <div
      aria-label={t("ownerView.tabs.label")}
      className="ml-3 flex shrink-0 items-center rounded-lg bg-muted p-0.5"
      data-testid="agent-view-tabs"
      role="tablist"
    >
      <button
        aria-selected={active === "chat"}
        className={tabClass(active === "chat")}
        data-testid="agent-view-tab-chat"
        onClick={() => {
          if (active !== "chat") void openDm();
        }}
        role="tab"
        type="button"
      >
        {t("ownerView.tabs.chat", { name: agentName })}
      </button>
      <button
        aria-selected={active === "conversations"}
        className={tabClass(active === "conversations")}
        data-testid="agent-view-tab-conversations"
        onClick={() => {
          if (active !== "conversations") void openDm(agentPubkey);
        }}
        role="tab"
        type="button"
      >
        {t("ownerView.tabs.conversations", { name: agentName })}
        {dots > 0 ? (
          <span className="ml-1.5 text-xs tabular-nums text-muted-foreground">
            {dots}
          </span>
        ) : null}
      </button>
    </div>
  );
}
