import * as React from "react";
import { toast } from "sonner";

import { useAppNavigation } from "@/app/navigation/useAppNavigation";
import { useOpenDmMutation } from "@/features/channels/hooks";
import { PeopleView } from "@/features/people/ui/PeopleView";
import {
  type ProfilePanelTab,
  type ProfilePanelView,
  UserProfilePanel,
} from "@/features/profile/ui/UserProfilePanel";
import {
  profilePanelTabFromSearch,
  profilePanelViewFromSearch,
} from "@/features/profile/ui/UserProfilePanelUtils";
import { useIdentityQuery } from "@/shared/api/hooks";
import { ProfilePanelProvider } from "@/shared/context/ProfilePanelContext";
import { useHistorySearchState } from "@/shared/hooks/useHistorySearchState";
import { useThreadPanelWidth } from "@/shared/hooks/useThreadPanelWidth";

const PEOPLE_PANEL_SEARCH_KEYS = [
  "profile",
  "profileTab",
  "profileView",
] as const;

/** People directory with the selected person's profile on the right. */
export function PeopleScreen() {
  const identityQuery = useIdentityQuery();
  const currentPubkey = identityQuery.data?.pubkey;
  const { applyPatch, values } = useHistorySearchState(
    PEOPLE_PANEL_SEARCH_KEYS,
  );
  const profilePanelPubkey = values.profile;
  const profilePanelTab = profilePanelTabFromSearch(values.profileTab);
  const profilePanelView = profilePanelViewFromSearch(values.profileView);
  const handleOpenProfilePanel = React.useCallback(
    (pubkey: string) =>
      applyPatch({ profile: pubkey, profileTab: null, profileView: null }),
    [applyPatch],
  );
  const handleCloseProfilePanel = React.useCallback(
    () => applyPatch({ profile: null, profileTab: null, profileView: null }),
    [applyPatch],
  );
  const handleProfilePanelViewChange = React.useCallback(
    (view: ProfilePanelView, options?: { replace?: boolean }) =>
      applyPatch({ profileView: view === "summary" ? null : view }, options),
    [applyPatch],
  );
  const handleProfilePanelTabChange = React.useCallback(
    (tab: ProfilePanelTab, options?: { replace?: boolean }) =>
      applyPatch({ profileTab: tab === "info" ? null : tab }, options),
    [applyPatch],
  );
  const threadPanelWidth = useThreadPanelWidth();
  const openDmMutation = useOpenDmMutation();
  const { goChannel } = useAppNavigation();
  // Same action as the profile popover's Message.
  const handleOpenDm = React.useCallback(
    async (pubkeys: string[]) => {
      const dm = await openDmMutation.mutateAsync({ pubkeys });
      await goChannel(dm.id);
    },
    [goChannel, openDmMutation],
  );
  const handleMessage = React.useCallback(
    (pubkey: string) => {
      handleOpenDm([pubkey]).catch((error: unknown) =>
        toast.error(
          error instanceof Error ? error.message : "Could not open the chat",
        ),
      );
    },
    [handleOpenDm],
  );

  return (
    <ProfilePanelProvider onOpenProfilePanel={handleOpenProfilePanel}>
      <div className="flex min-h-0 min-w-0 flex-1 flex-row overflow-hidden">
        <div className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden">
          <PeopleView
            currentPubkey={currentPubkey}
            onMessage={handleMessage}
            onOpenProfile={handleOpenProfilePanel}
            selectedPubkey={profilePanelPubkey ?? null}
          />
        </div>
        {profilePanelPubkey ? (
          <UserProfilePanel
            canResetWidth={threadPanelWidth.canReset}
            currentPubkey={currentPubkey}
            onClose={handleCloseProfilePanel}
            onOpenDm={handleOpenDm}
            onOpenProfile={handleOpenProfilePanel}
            onResetWidth={threadPanelWidth.onResetWidth}
            onResizeStart={threadPanelWidth.onResizeStart}
            onTabChange={handleProfilePanelTabChange}
            onViewChange={handleProfilePanelViewChange}
            pubkey={profilePanelPubkey}
            tab={profilePanelTab}
            view={profilePanelView}
            widthPx={threadPanelWidth.widthPx}
          />
        ) : null}
      </div>
    </ProfilePanelProvider>
  );
}
