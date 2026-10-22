import { toast } from "sonner";

import { attachManagedAgentToChannel } from "./channelAgents";
import type { Channel, CreateManagedAgentResponse } from "@/shared/api/types";
import { t } from "@/shared/i18n";

type TargetChannel = Pick<Channel, "id" | "name">;

async function attach(
  created: CreateManagedAgentResponse,
  targetChannel: TargetChannel,
) {
  const attached = await attachManagedAgentToChannel(targetChannel.id, {
    agent: created.agent,
    role: "bot",
    ensureRunning: true,
  });
  created.agent = attached.agent;
}

function showAttachmentFailure(
  created: CreateManagedAgentResponse,
  targetChannel: TargetChannel,
  cause: unknown,
  toastId?: string | number,
) {
  const error =
    cause instanceof Error ? cause.message : t("agents.addAgent.failed");
  const id = toast.warning(t("agents.created.title"), {
    description: t("agents.created.attachFailed", {
      name: created.agent.name,
      group: targetChannel.name,
      error,
    }),
    id: toastId,
    action: {
      label: t("agents.created.tryAgain"),
      onClick: (event) => {
        event.preventDefault();
        toast.loading(t("agents.created.title"), {
          description: t("agents.created.adding", {
            name: created.agent.name,
            group: targetChannel.name,
          }),
          id,
        });
        void attach(created, targetChannel).then(
          () => {
            toast.success(t("agents.created.title"), {
              description: t("agents.created.added", {
                name: created.agent.name,
                group: targetChannel.name,
              }),
              id,
            });
          },
          (retryCause: unknown) => {
            showAttachmentFailure(created, targetChannel, retryCause, id);
          },
        );
      },
    },
  });
}

/** Keeps creation successful when its optional channel attachment fails. */
export function useCreatedAgentChannelAttachment() {
  async function presentCreatedAgent(
    created: CreateManagedAgentResponse,
    targetChannel?: TargetChannel | null,
  ) {
    if (created.spawnError || !targetChannel) {
      toast.success(t("agents.created.title"));
      return;
    }

    try {
      await attach(created, targetChannel);
      toast.success(t("agents.created.title"));
    } catch (cause) {
      showAttachmentFailure(created, targetChannel, cause);
    }
  }

  return { presentCreatedAgent };
}
