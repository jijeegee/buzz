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
import { StatusEmoji } from "@/features/user-status/ui/StatusEmoji";
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
import { cn } from "@/shared/lib/cn";

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
  const selfProfileCache = useSelfProfileCache();
  const myMembershipQuery = useMyRelayMembershipLookupQuery();
  const activeRole = myMembershipQuery.data?.membership?.role;
  const canInvite = activeRole === "owner" || activeRole === "admin";
  // One built-in server: no server name, switcher or server actions; the
  // sidebar connection card still reports connection problems.
  const singleServer = useBuiltInServer() !== null;
  const [profilePopoverOpen, setProfilePopoverOpen] = React.useState(false);
  const profileCardRef = React.useRef<HTMLDivElement | null>(null);
  const accountMenu = useAccountMenu({ open: profilePopoverOpen });
  const activePubkey = useIdentityQuery().data?.pubkey ?? null;
  const watchedUnread = useWatchedUnread();
  const otherAccountUnread = anyWatchedUnread(watchedUnread);
  useRememberAccountKuzzName(activePubkey, profile?.displayName);
  const toggleProfilePopover = React.useCallback(
    () => setProfilePopoverOpen((prev) => !prev),
    [],
  );
  const handleCardClick = React.useCallback(
    (event: React.MouseEvent<HTMLDivElement>) => {
      const target = event.target;
      if (
        !(target instanceof Node) ||
        !profileCardRef.current?.contains(target)
      ) {
        return;
      }
      toggleProfilePopover();
    },
    [toggleProfilePopover],
  );
  const hasStatus = Boolean(selfUserStatus?.text || selfUserStatus?.emoji);
  const communityLabel = activeCommunity?.name ?? "No community";
  const readonlyCommunityLabel = (
    <span
      className="flex min-w-0 cursor-pointer items-center gap-1 text-xs leading-snug text-sidebar-foreground/70"
      data-kuzz-sidebar-secondary
    >
      <span
        aria-hidden="true"
        className="flex w-3.5 shrink-0 items-center justify-center text-2xs"
      >
        <span className="-translate-y-px leading-normal">🐝</span>
      </span>
      <span className="truncate">{communityLabel}</span>
    </span>
  );

  return (
    // biome-ignore lint/a11y/noStaticElementInteractions lint/a11y/useKeyWithClickEvents: child buttons provide keyboard access; wrapper fills pointer gaps between them.
    <div
      className="group/profile-card cursor-pointer rounded-xl px-2 py-2 transition-colors hover:bg-sidebar-border/35"
      data-testid="sidebar-profile-card"
      onClick={handleCardClick}
      ref={profileCardRef}
    >
      <div className="flex min-w-0 items-center gap-3">
        <button
          aria-label={`Open profile menu for ${resolvedDisplayName}`}
          className="relative shrink-0 rounded-xl outline-hidden focus:outline-none focus-visible:outline-none"
          data-testid="sidebar-profile-avatar-button"
          onClick={(event) => {
            event.stopPropagation();
            toggleProfilePopover();
          }}
          type="button"
        >
          <MaskedAvatarBadgeFrame
            badge={
              <span
                aria-label={getPresenceLabel(selfPresenceStatus)}
                className="flex h-3.5 w-3.5 items-center justify-center rounded-full"
                data-testid="self-presence-badge"
                role="img"
              >
                <PresenceDot className="h-2 w-2" status={selfPresenceStatus} />
              </span>
            }
            badgeBox={{ bottom: -2, height: 14, right: -2, width: 14 }}
            className="h-8 w-8"
            curve={STATUS_DOT_MASK_CURVE}
            cutout={{ cx: 28, cy: 28, r: 7.5 }}
            size={32}
          >
            <ProfileAvatar
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
                Another account has unread messages
              </span>
            </span>
          ) : null}
        </button>

        <div className="min-w-0 flex-1">
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
            triggerContainerRef={profileCardRef}
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
              onClick={(event) => {
                event.stopPropagation();
                toggleProfilePopover();
              }}
              className="block w-full min-w-0 rounded-sm text-left text-sidebar-foreground outline-hidden focus:outline-none focus-visible:outline-none"
              data-testid="open-settings"
              type="button"
            >
              <p
                className="truncate text-sm font-semibold leading-tight text-current"
                data-testid="sidebar-profile-name"
              >
                {resolvedDisplayName}
              </p>
            </button>
          </ProfilePopover>

          {singleServer ? (
            hasStatus ? (
              <div className="relative mt-0.5">
                <button
                  aria-label={`Open profile menu for ${resolvedDisplayName}`}
                  className="flex w-full min-w-0 items-center truncate rounded-sm text-left text-xs leading-snug text-sidebar-foreground/70 outline-hidden focus:outline-none focus-visible:outline-none"
                  data-kuzz-sidebar-secondary
                  data-testid="sidebar-profile-user-status"
                  onClick={(event) => {
                    event.stopPropagation();
                    toggleProfilePopover();
                  }}
                  type="button"
                >
                  {selfUserStatus?.emoji ? (
                    <StatusEmoji
                      className="mr-1 w-4 shrink-0 text-xs"
                      value={selfUserStatus.emoji}
                    />
                  ) : null}
                  <span className="truncate">{selfUserStatus?.text}</span>
                </button>
              </div>
            ) : null
          ) : hasStatus ? (
            <div className="relative mt-0.5">
              <button
                aria-label={`Open profile menu for ${resolvedDisplayName}`}
                className={cn(
                  "flex w-full min-w-0 items-center truncate rounded-sm text-left text-xs leading-snug text-sidebar-foreground/70 outline-hidden transition-opacity duration-150 focus:outline-none focus-visible:outline-none group-hover/profile-card:opacity-0",
                  profilePopoverOpen && "opacity-100",
                )}
                data-kuzz-sidebar-secondary
                data-testid="sidebar-profile-user-status"
                onClick={(event) => {
                  event.stopPropagation();
                  toggleProfilePopover();
                }}
                type="button"
              >
                {selfUserStatus?.emoji ? (
                  <StatusEmoji
                    className="mr-1 w-4 shrink-0 text-xs"
                    value={selfUserStatus.emoji}
                  />
                ) : null}
                <span className="truncate">{selfUserStatus?.text}</span>
              </button>
              <div
                className={cn(
                  "pointer-events-none absolute inset-0 flex min-w-0 items-center text-xs leading-snug text-sidebar-foreground/70 opacity-0 transition-opacity duration-150 group-hover/profile-card:opacity-100",
                  profilePopoverOpen && "opacity-0",
                )}
                data-kuzz-sidebar-secondary
              >
                {readonlyCommunityLabel}
              </div>
            </div>
          ) : (
            <div className="relative mt-0.5">{readonlyCommunityLabel}</div>
          )}
        </div>
      </div>
      <CreateAccountDialog login={accountMenu.signupPrompt} />
    </div>
  );
}
