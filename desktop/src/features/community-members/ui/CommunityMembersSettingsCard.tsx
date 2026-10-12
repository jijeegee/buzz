import { Crown, MoreHorizontal, Search, Shield } from "lucide-react";
import { nip19 } from "nostr-tools";
import * as React from "react";
import { toast } from "sonner";

import {
  useChangeRelayMemberRoleMutation,
  useMyRelayMembershipLookupQuery,
  useRelayMembersQuery,
  useRemoveRelayMemberMutation,
} from "@/features/community-members/hooks";
import { useUsersBatchQuery } from "@/features/profile/hooks";
import { ProfileAvatar } from "@/features/profile/ui/ProfileAvatar";
import { UserProfilePopover } from "@/features/profile/ui/UserProfilePopover";
import { SettingsConfirmDialog } from "@/features/settings/ui/SettingsConfirmDialog";
import { SettingsOptionGroup } from "@/features/settings/ui/SettingsOptionGroup";
import { SettingsSectionHeader } from "@/features/settings/ui/SettingsSectionHeader";
import type {
  RelayMember,
  RelayMemberRole,
  UserProfileSummary,
} from "@/shared/api/types";
import { normalizePubkey, UNKNOWN_USER_LABEL } from "@/shared/lib/pubkey";
import { Button } from "@/shared/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/shared/ui/dropdown-menu";
import { VirtualizedList } from "@/shared/ui/VirtualizedList";
import { CommunityInviteDialog } from "./CommunityInviteDialog";

function formatDisplayName(member: RelayMember, displayName?: string | null) {
  const trimmedDisplayName = displayName?.trim();
  if (
    trimmedDisplayName &&
    !trimmedDisplayName.toLowerCase().startsWith("npub1")
  ) {
    return trimmedDisplayName;
  }
  return member.role === "owner" ? "Community owner" : UNKNOWN_USER_LABEL;
}

function npubFromPubkey(pubkey: string): string | null {
  try {
    return nip19.npubEncode(pubkey);
  } catch {
    return null;
  }
}

function formatDate(dateString: string): string {
  return new Date(dateString).toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
    year: "numeric",
  });
}

function MemberIdentity({
  displayName,
  pubkey,
}: {
  displayName: string;
  pubkey: string;
}) {
  return (
    <span
      className="min-w-0 max-w-40 truncate"
      data-testid={`relay-member-name-${pubkey}`}
    >
      {displayName}
    </span>
  );
}

function RelayMemberRow({
  currentRole,
  currentPubkey,
  profile,
  member,
}: {
  currentRole: RelayMemberRole;
  currentPubkey?: string;
  profile?: UserProfileSummary;
  member: RelayMember;
}) {
  const removeMutation = useRemoveRelayMemberMutation();
  const changeRoleMutation = useChangeRelayMemberRoleMutation();
  const isSelf = currentPubkey
    ? normalizePubkey(currentPubkey) === normalizePubkey(member.pubkey)
    : false;
  const isBusy = removeMutation.isPending || changeRoleMutation.isPending;
  const canRemove =
    !isSelf &&
    member.role !== "owner" &&
    (currentRole === "owner" || member.role === "member");
  const canPromote = currentRole === "owner" && member.role === "member";
  const canDemote = currentRole === "owner" && member.role === "admin";
  const hasActions = canRemove || canPromote || canDemote;
  const displayName = formatDisplayName(member, profile?.displayName);
  const [confirmRemove, setConfirmRemove] = React.useState(false);

  async function mutateWithToast(
    action: () => Promise<unknown>,
    success: string,
  ) {
    try {
      await action();
      toast.success(success);
    } catch (error) {
      toast.error(
        error instanceof Error
          ? error.message
          : "Couldn’t update this community member.",
      );
    }
  }

  return (
    <div
      className="group/member flex min-h-14 items-center gap-3 px-1 py-2.5"
      data-testid={`relay-member-row-${member.pubkey}`}
    >
      <UserProfilePopover
        pubkey={member.pubkey}
        triggerAriaLabel={`Open profile for ${displayName}`}
        triggerElement="span"
      >
        <ProfileAvatar
          avatarUrl={profile?.avatarUrl ?? null}
          className="h-9 w-9 text-xs shadow-none"
          label={displayName}
          shape={profile?.isAgent === true ? "squircle" : "circle"}
        />
      </UserProfilePopover>
      <div className="min-w-0 flex-1">
        <div className="flex min-w-0 items-center gap-1.5 text-sm font-medium">
          <MemberIdentity displayName={displayName} pubkey={member.pubkey} />
          {member.role === "owner" ? (
            <Crown className="h-4 w-4 text-amber-500" />
          ) : null}
          {member.role === "admin" ? (
            <Shield className="h-4 w-4 text-blue-500" />
          ) : null}
        </div>
        <div
          className="flex items-center gap-1.5 text-xs text-muted-foreground/70"
          data-settings-subcopy
        >
          <span className="shrink-0 capitalize">{member.role}</span>
          <span aria-hidden="true" className="shrink-0">
            ·
          </span>
          <span className="shrink-0">Added {formatDate(member.createdAt)}</span>
          {isSelf ? (
            <>
              <span aria-hidden="true" className="shrink-0">
                ·
              </span>
              <span className="shrink-0">You</span>
            </>
          ) : null}
        </div>
      </div>

      {hasActions ? (
        <DropdownMenu modal={false}>
          <DropdownMenuTrigger asChild>
            <Button
              aria-label={`Actions for ${displayName}`}
              data-testid={`relay-member-actions-${member.pubkey}`}
              disabled={isBusy}
              size="icon"
              variant="ghost"
            >
              <MoreHorizontal className="h-4 w-4" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            {canPromote ? (
              <DropdownMenuItem
                onClick={() =>
                  void mutateWithToast(
                    () =>
                      changeRoleMutation.mutateAsync({
                        pubkey: member.pubkey,
                        role: "admin",
                      }),
                    "Made community admin",
                  )
                }
              >
                Make admin
              </DropdownMenuItem>
            ) : null}
            {canDemote ? (
              <DropdownMenuItem
                onClick={() =>
                  void mutateWithToast(
                    () =>
                      changeRoleMutation.mutateAsync({
                        pubkey: member.pubkey,
                        role: "member",
                      }),
                    "Made community member",
                  )
                }
              >
                Make member
              </DropdownMenuItem>
            ) : null}
            {canRemove && (canPromote || canDemote) ? (
              <DropdownMenuSeparator />
            ) : null}
            {canRemove ? (
              <DropdownMenuItem
                className="text-destructive focus:text-destructive"
                onClick={() => setConfirmRemove(true)}
              >
                Remove from community
              </DropdownMenuItem>
            ) : null}
          </DropdownMenuContent>
        </DropdownMenu>
      ) : null}
      {canRemove ? (
        <SettingsConfirmDialog
          confirmLabel="Remove"
          description={`${displayName} will lose access to this community's channels and conversations. You can invite them again later.`}
          destructive
          onConfirm={() =>
            void mutateWithToast(
              () => removeMutation.mutateAsync(member.pubkey),
              "Removed community member",
            )
          }
          onOpenChange={setConfirmRemove}
          open={confirmRemove}
          testId="relay-member-remove-dialog"
          title={`Remove ${displayName}?`}
        />
      ) : null}
    </div>
  );
}

export function CommunityMembersSettingsCard({
  currentPubkey,
}: {
  currentPubkey?: string;
}) {
  const myMembershipQuery = useMyRelayMembershipLookupQuery();
  const currentRole = myMembershipQuery.data?.membership?.role ?? null;
  const canManageRelay = currentRole === "owner" || currentRole === "admin";
  const membersQuery = useRelayMembersQuery(canManageRelay);
  const members = React.useMemo(
    () => membersQuery.data ?? [],
    [membersQuery.data],
  );
  const profilesQuery = useUsersBatchQuery(
    members.map((member) => member.pubkey),
    {
      enabled: canManageRelay && members.length > 0,
    },
  );
  const profiles = profilesQuery.data?.profiles;
  const [inviteDialogOpen, setInviteDialogOpen] = React.useState(false);
  const [search, setSearch] = React.useState("");

  const filteredMembers = React.useMemo(() => {
    const q = search.trim().toLowerCase();
    if (!q) return members;
    return members.filter((member) => {
      const normalizedPubkey = normalizePubkey(member.pubkey);
      const profile = profiles?.[normalizedPubkey];
      const displayName = profile?.displayName?.toLowerCase() ?? "";
      const nip05 = profile?.nip05Handle?.toLowerCase() ?? "";
      const npub = npubFromPubkey(member.pubkey)?.toLowerCase() ?? "";
      return (
        displayName.includes(q) ||
        nip05.includes(q) ||
        npub.includes(q) ||
        member.pubkey.toLowerCase().includes(q) ||
        member.role.includes(q)
      );
    });
  }, [members, profiles, search]);

  if (myMembershipQuery.isLoading) {
    return (
      <section className="min-w-0" data-testid="settings-community-members">
        <p className="text-sm text-muted-foreground">
          Checking invite permissions…
        </p>
      </section>
    );
  }

  if (!canManageRelay || !currentRole) {
    return null;
  }

  return (
    <section className="min-w-0" data-testid="settings-community-members">
      <SettingsSectionHeader
        action={
          <Button
            data-testid="community-invite-dialog-trigger"
            onClick={() => setInviteDialogOpen(true)}
          >
            Invite to community
          </Button>
        }
        title="Invites"
        description="Manage members and community access."
      />

      <SettingsOptionGroup
        title={
          <>
            Members
            {members.length > 0 ? (
              <span className="ml-1.5 font-normal">{members.length}</span>
            ) : null}
          </>
        }
      >
        <div className="space-y-3 p-4 sm:p-5">
          <div className="relative">
            <Search className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
            <input
              autoCapitalize="none"
              autoCorrect="off"
              className="w-full rounded-lg border border-border/70 bg-background py-2 pl-9 pr-3 text-sm placeholder:text-muted-foreground focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring"
              data-testid="community-members-search"
              onChange={(event) => setSearch(event.target.value)}
              placeholder="Search members"
              spellCheck={false}
              type="text"
              value={search}
            />
          </div>

          {membersQuery.error instanceof Error ? (
            <p className="rounded-md border border-destructive/30 bg-destructive/10 px-3 py-2 text-sm text-destructive">
              {membersQuery.error.message}
            </p>
          ) : null}

          {membersQuery.isLoading ? (
            <p className="py-3 text-sm text-muted-foreground">
              Loading community members…
            </p>
          ) : members.length === 0 ? (
            <p className="rounded-lg border border-dashed border-border/70 px-3 py-6 text-center text-sm text-muted-foreground">
              No community members yet.
            </p>
          ) : filteredMembers.length === 0 ? (
            <p className="rounded-lg border border-dashed border-border/70 px-3 py-6 text-center text-sm text-muted-foreground">
              No members match your search.
            </p>
          ) : (
            <VirtualizedList
              className="max-h-[28rem] divide-y divide-border/60"
              estimateSize={60}
              getItemKey={(member) => member.pubkey}
              items={filteredMembers}
              renderItem={(member) => (
                <RelayMemberRow
                  currentPubkey={currentPubkey}
                  currentRole={currentRole}
                  member={member}
                  profile={profiles?.[normalizePubkey(member.pubkey)]}
                />
              )}
            />
          )}
        </div>
      </SettingsOptionGroup>

      <CommunityInviteDialog
        isOwner={currentRole === "owner"}
        onOpenChange={setInviteDialogOpen}
        open={inviteDialogOpen}
      />
    </section>
  );
}
