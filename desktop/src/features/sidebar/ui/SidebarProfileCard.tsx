import * as React from "react";

import { useAccountMenu } from "@/features/accounts/useAccountMenu";
import { useRememberAccountKuzzName } from "@/features/accounts/useRememberAccountKuzzName";
import { AccountSwitcherSection } from "@/features/accounts/ui/AccountSwitcherSection";
import {
  anyWatchedUnread,
  useWatchedUnread,
} from "@/features/accounts/inactiveAccountWatch/useWatchedUnread";
import { getPresenceLabel } from "@/features/presence/lib/presence";
import { PresenceDot } from "@/features/presence/ui/PresenceBadge";
import { useSelfProfileCache } from "@/features/profile/hooks";
import { ProfileAvatar } from "@/features/profile/ui/ProfileAvatar";
import {
  MaskedAvatarBadgeFrame,
  STATUS_DOT_MASK_CURVE,
} from "@/features/profile/ui/MaskedAvatarBadgeFrame";
import { ProfilePopover } from "@/features/profile/ui/ProfilePopover";
import { CreateAccountDialog } from "@/shared/ui/CreateAccountDialog";
import type { UserStatusInput } from "@/features/user-status/types";
import type { LeaveCommunityResult } from "@/features/communities/leaveCommunity";
import type { Community } from "@/features/communities/types";
import { useBuiltInServer } from "@/features/communities/builtInServer";
import { CommunitySwitcher } from "@/features/communities/ui/CommunitySwitcher";
import { useMyRelayMembershipLookupQuery } from "@/features/community-members/hooks";
import type { SettingsSection } from "@/features/settings/ui/SettingsPanels";
import type { PresenceStatus, Profile, UserStatus } from "@/shared/api/types";
import { useIdentityQuery } from "@/shared/api/hooks";
import { useT } from "@/shared/i18n";

type SidebarProfileCardProps = {
  activeCommunity: Community | null;
  isPresencePending?: boolean;
  onOpenAddCommunity: () => void;
  onOpenSettings: (section?: SettingsSection) => void;
  onLeaveCommunity: (id: string) => Promise<LeaveCommunityResult | undefined>;
  onRemoveCommunityFromDevice: (id: string) => Promise<void>;
  onSendFeedback?: () => void;
  onSetPresenceStatus?: (status: PresenceStatus) => void;
  onSetUserStatus: (status: UserStatusInput) => void;
  onClearUserStatus: () => void;
  onSwitchCommunity: (id: string) => void;
  onUpdateCommunity: (
    id: string,
    updates: Partial<Pick<Community, "name" | "relayUrl" | "token">>,
  ) => void;
  profile?: Profile;
  resolvedDisplayName: string;
  selfPresenceStatus: PresenceStatus;
  selfUserStatus?: UserStatus;
  communities: Community[];
};

export function SidebarProfileCard({
  activeCommunity,
  isPresencePending,
  onOpenAddCommunity,
  onOpenSettings,
  onSendFeedback,
  onLeaveCommunity,
  onRemoveCommunityFromDevice,
  onSetPresenceStatus,
  onSetUserStatus,
  onClearUserStatus,
  onSwitchCommunity,
  onUpdateCommunity,
  profile,
  resolvedDisplayName,
  selfPresenceStatus,
  selfUserStatus,
  communities,
}: SidebarProfileCardProps) {
  const t = useT();
  const selfProfileCache = useSelfProfileCache();
  const myMembershipQuery = useMyRelayMembershipLookupQuery();
  const activeRole = myMembershipQuery.data?.membership?.role;
  const canInvite = activeRole === "owner" || activeRole === "admin";
  // One built-in server: no server name, switcher or server actions; the
  // sidebar connection card still reports connection problems.
  const singleServer = useBuiltInServer() !== null;
  const [profilePopoverOpen, setProfilePopoverOpen] = React.useState(false);
  const accountMenu = useAccountMenu({ open: profilePopoverOpen });
  const activePubkey = useIdentityQuery().data?.pubkey ?? null;
  const watchedUnread = useWatchedUnread();
  const otherAccountUnread = anyWatchedUnread(watchedUnread);
  useRememberAccountKuzzName(activePubkey, profile?.displayName);
  const statusLabel = [selfUserStatus?.emoji, selfUserStatus?.text]
    .filter(Boolean)
    .join(" ");

  return (
    // The profile sits at the bottom of the left icon rail as just the
    // avatar; its menu opens to the right of it.
    <div
      className="flex w-full justify-center"
      data-testid="sidebar-profile-card"
    >
      <ProfilePopover
        open={profilePopoverOpen}
        onOpenChange={setProfilePopoverOpen}
        avatarDataUrl={selfProfileCache?.avatarDataUrl ?? null}
        avatarUrl={profile?.avatarUrl ?? null}
        currentStatus={selfPresenceStatus}
        displayName={resolvedDisplayName}
        isStatusPending={isPresencePending}
        onClearUserStatus={onClearUserStatus}
        onOpenSettings={onOpenSettings}
        onSendFeedback={onSendFeedback}
        onSetStatus={onSetPresenceStatus ?? (() => {})}
        onSetUserStatus={onSetUserStatus}
        userStatusEmoji={selfUserStatus?.emoji}
        userStatusExpiresAt={selfUserStatus?.expiresAt}
        userStatusText={selfUserStatus?.text}
        userStatusUpdatedAt={selfUserStatus?.updatedAt}
        accountsSlot={
          <AccountSwitcherSection
            accounts={accountMenu.accounts}
            busy={accountMenu.busy}
            unread={watchedUnread}
            onSignIn={() => {
              setProfilePopoverOpen(false);
              void accountMenu.signIn(activePubkey);
            }}
            onSignInWithApple={
              accountMenu.appleSignIn
                ? () => {
                    setProfilePopoverOpen(false);
                    void accountMenu.signIn(activePubkey, "apple");
                  }
                : undefined
            }
            onSwitch={(account) => {
              setProfilePopoverOpen(false);
              void accountMenu.switchTo(account);
            }}
          />
        }
        onInvitePeople={
          singleServer && canInvite
            ? () => onOpenSettings("community-members")
            : undefined
        }
        communitySwitcherSlot={
          singleServer ? undefined : (
            <CommunitySwitcher
              activeCommunity={activeCommunity}
              canInvite={canInvite}
              onAddCommunity={() => {
                setProfilePopoverOpen(false);
                onOpenAddCommunity();
              }}
              onInvite={() => {
                setProfilePopoverOpen(false);
                onOpenSettings("community-members");
              }}
              onLeaveCommunity={onLeaveCommunity}
              onRemoveCommunityFromDevice={onRemoveCommunityFromDevice}
              onSwitchCommunity={onSwitchCommunity}
              onUpdateCommunity={onUpdateCommunity}
              variant="profile-menu"
              communities={communities}
            />
          )
        }
      >
        <button
          aria-label={t("sidebar.profileCard.openMenu", {
            name: resolvedDisplayName,
          })}
          className="relative flex items-center justify-center rounded-xl p-1.5 outline-hidden transition-colors hover:bg-sidebar-accent focus:outline-none focus-visible:ring-1 focus-visible:ring-ring"
          data-testid="open-settings"
          title={
            statusLabel
              ? `${resolvedDisplayName} · ${statusLabel}`
              : resolvedDisplayName
          }
          type="button"
        >
          <span
            className="relative block"
            data-testid="sidebar-profile-avatar-button"
          >
            <MaskedAvatarBadgeFrame
              badge={
                <span
                  aria-label={getPresenceLabel(selfPresenceStatus)}
                  className="flex h-3.5 w-3.5 items-center justify-center rounded-full"
                  data-testid="self-presence-badge"
                  role="img"
                >
                  <PresenceDot
                    className="h-2 w-2"
                    status={selfPresenceStatus}
                  />
                </span>
              }
              badgeBox={{ bottom: -2, height: 14, right: -2, width: 14 }}
              className="h-8 w-8"
              curve={STATUS_DOT_MASK_CURVE}
              cutout={{ cx: 28, cy: 28, r: 7.5 }}
              size={32}
            >
              <ProfileAvatar
                mediaVariant="small"
                avatarDataUrl={selfProfileCache?.avatarDataUrl ?? null}
                avatarUrl={profile?.avatarUrl ?? null}
                className="h-full w-full text-xs"
                iconClassName="h-4 w-4"
                label={resolvedDisplayName}
                testId="sidebar-profile-avatar"
              />
            </MaskedAvatarBadgeFrame>
            {otherAccountUnread ? (
              <span
                className="absolute -right-0.5 -top-0.5 h-2 w-2 rounded-full bg-primary ring-2 ring-sidebar"
                data-testid="sidebar-other-account-unread"
              >
                <span className="sr-only">
                  {t("sidebar.profileCard.otherAccountUnread")}
                </span>
              </span>
            ) : null}
          </span>
          <span className="sr-only" data-testid="sidebar-profile-name">
            {resolvedDisplayName}
          </span>
        </button>
      </ProfilePopover>
      <CreateAccountDialog login={accountMenu.signupPrompt} />
    </div>
  );
}
