import { useMutation, useQueryClient } from "@tanstack/react-query";
import * as React from "react";
import { toast } from "sonner";

import { AgentCreationPreview } from "@/features/agents/ui/AgentCreationPreview";
import { downscaleIconToDataUrl } from "@/features/communities/lib/downscaleIcon";
import {
  communityIconQueryKey,
  useActiveCommunityIcon,
} from "@/features/communities/useCommunityIcons";
import { useBuiltInServer } from "@/features/communities/builtInServer";
import { useCommunities } from "@/features/communities/useCommunities";
import { useMyRelayMembershipLookupQuery } from "@/features/community-members/hooks";
import { setCommunityIcon } from "@/shared/api/communityProfile";

/**
 * Community icon editor using the same image/emoji picker as agent creation.
 * Images stay as small inline data URLs so inactive-community rails can render
 * them without crossing another relay's authenticated media boundary.
 */
export function CommunityIconSettingsCard({
  compact = false,
}: {
  compact?: boolean;
}) {
  const { activeCommunity } = useCommunities();
  const singleServer = useBuiltInServer() !== null;
  const relayUrl = activeCommunity?.relayUrl;
  const iconQuery = useActiveCommunityIcon(relayUrl);
  const queryClient = useQueryClient();
  const persistedIcon = iconQuery.data ?? "";
  const [draftIcon, setDraftIcon] = React.useState(persistedIcon);
  const queuedIconRef = React.useRef<string | undefined>(undefined);
  const isSavingRef = React.useRef(false);
  const hasLocalEditRef = React.useRef(false);

  React.useEffect(() => {
    if (hasLocalEditRef.current) return;
    setDraftIcon(persistedIcon);
  }, [persistedIcon]);

  const mutation = useMutation({
    mutationFn: (icon: string) => setCommunityIcon(icon),
    onSuccess: async (_data, icon) => {
      if (!relayUrl) return;
      queryClient.setQueryData(communityIconQueryKey(relayUrl), icon || null);
      await queryClient.invalidateQueries({
        queryKey: communityIconQueryKey(relayUrl),
      });
    },
  });

  const mutateIcon = mutation.mutateAsync;

  const persistIcon = React.useCallback(
    (icon: string) => {
      hasLocalEditRef.current = true;
      queuedIconRef.current = icon;
      setDraftIcon(icon);
      if (isSavingRef.current) return;

      isSavingRef.current = true;
      const drainQueue = async () => {
        let confirmedIcon = persistedIcon;
        let lastAttemptFailed = false;
        while (queuedIconRef.current !== undefined) {
          const nextIcon = queuedIconRef.current;
          queuedIconRef.current = undefined;
          try {
            await mutateIcon(nextIcon);
            confirmedIcon = nextIcon;
            lastAttemptFailed = false;
          } catch (error) {
            lastAttemptFailed = true;
            toast.error(
              error instanceof Error
                ? error.message
                : "Couldn’t update the community icon.",
            );
          }
        }
        if (lastAttemptFailed) setDraftIcon(confirmedIcon);
        hasLocalEditRef.current = false;
        isSavingRef.current = false;
      };
      void drainQueue();
    },
    [mutateIcon, persistedIcon],
  );

  function previewIcon(icon: string) {
    hasLocalEditRef.current = true;
    setDraftIcon(icon);
  }

  function clearIconPreview() {
    hasLocalEditRef.current = true;
    setDraftIcon("");
  }

  return (
    <div
      className={
        compact
          ? "flex shrink-0 items-center"
          : "flex min-w-0 flex-col items-center"
      }
      data-testid="community-icon-settings"
    >
      <AgentCreationPreview
        assetLabel="community icon"
        avatarUrl={draftIcon || null}
        label={singleServer ? "Icon" : (activeCommunity?.name ?? "Community")}
        onClearAvatar={clearIconPreview}
        onCommitAvatar={persistIcon}
        onSelectAvatar={previewIcon}
        processImage={downscaleIconToDataUrl}
        shape="rounded-square"
        testIdPrefix="community-icon"
        variant={compact ? "compact" : "default"}
      />
    </div>
  );
}

/**
 * Who may edit the server-wide icon; the same rule Edit Community uses: owners
 * and admins, or anyone on a relay without membership.
 */
export function canEditCommunityIcon(
  membership:
    | { membershipRequired?: boolean; membership?: { role?: string } | null }
    | null
    | undefined,
): boolean {
  const role = membership?.membership?.role;
  return (
    membership?.membershipRequired === false ||
    role === "owner" ||
    role === "admin"
  );
}

/**
 * Settings › Members: the icon editor that used to live in Edit Community,
 * shown only with a built-in server (Edit Community is gone there) and only
 * to members who may edit it.
 */
export function MembersIconSettingsSection() {
  const singleServer = useBuiltInServer() !== null;
  const membershipQuery = useMyRelayMembershipLookupQuery();
  if (!singleServer || !canEditCommunityIcon(membershipQuery.data)) {
    return null;
  }
  return (
    <div
      className="mb-6 flex items-center justify-between gap-4 rounded-xl border border-border/70 bg-muted/20 p-3"
      data-testid="members-icon-settings"
    >
      <div className="min-w-0">
        <p className="text-sm font-medium">Icon</p>
        <p className="text-xs text-muted-foreground">
          Shown to everyone who signs in.
        </p>
      </div>
      <CommunityIconSettingsCard compact />
    </div>
  );
}
