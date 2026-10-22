import type { ReactNode } from "react";

import { useAppNavigation } from "@/app/navigation/useAppNavigation";
import {
  isChannelReferenceOpenable,
  useChannelReference,
} from "@/features/channels/openChannelDirectory";
import { useT } from "@/shared/i18n";

export function ProjectOriginReference({
  agentName,
  channelId,
}: {
  agentName?: string | null;
  channelId?: string | null;
}) {
  const { goChannel } = useAppNavigation();
  const channel = useChannelReference(channelId);
  const t = useT();

  if (channelId) {
    const isOpenable = isChannelReferenceOpenable(channel);
    return (
      <span
        className="inline-flex max-w-full min-w-0 items-center gap-1"
        title={
          isOpenable
            ? t("projects.origin.unverified")
            : // Resolved via a bounded per-id lookup ({@link useChannelReference}),
              // so an unresolved id is a private, deleted, or otherwise
              // inaccessible channel — a non-member open channel still names it.
              t("projects.origin.hiddenGroup", { id: channelId })
        }
      >
        {t(
          isOpenable
            ? "projects.origin.startedFrom"
            : "projects.origin.startedFromPrivate",
          {
            name: channel?.name ?? "",
            phrase: (chunks: ReactNode) => (
              <span
                className="shrink-0 whitespace-nowrap"
                data-project-metadata-phrase
              >
                {chunks}
              </span>
            ),
            group: (chunks: ReactNode) => (
              <button
                aria-label={t("projects.origin.openGroup", {
                  name: channel?.name ?? "",
                })}
                className="min-w-0 truncate font-medium text-foreground underline-offset-2 hover:underline"
                onClick={() => {
                  if (channel) void goChannel(channel.id);
                }}
                type="button"
              >
                {chunks}
              </button>
            ),
            private: (chunks: ReactNode) => (
              <span className="shrink-0 whitespace-nowrap">{chunks}</span>
            ),
          },
        )}
      </span>
    );
  }

  if (agentName) {
    return (
      <span
        className="inline-flex max-w-full min-w-0 items-center gap-1"
        title={t("projects.origin.privateOmitted")}
      >
        {t("projects.origin.startedPrivately", {
          name: agentName,
          phrase: (chunks: ReactNode) => (
            <span
              className="shrink-0 whitespace-nowrap"
              data-project-metadata-phrase
            >
              {chunks}
            </span>
          ),
          agent: (chunks: ReactNode) => (
            <span className="truncate font-medium text-foreground">
              {chunks}
            </span>
          ),
        })}
      </span>
    );
  }

  return null;
}
