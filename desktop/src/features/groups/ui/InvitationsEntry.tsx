import * as React from "react";
import { useQueryClient } from "@tanstack/react-query";
import { ChevronRight, MailPlus } from "lucide-react";
import { toast } from "sonner";

import { channelsQueryKey } from "@/features/channels/hooks";
import { GroupLetterAvatar } from "@/features/home/ui/ConversationLetterAvatar";
import { useUsersBatchQuery } from "@/features/profile/hooks";
import { resolveUserLabel } from "@/features/profile/lib/identity";
import { useIdentityQuery } from "@/shared/api/hooks";
import { Button } from "@/shared/ui/button";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/shared/ui/dialog";
import {
  acceptGroupInvitation,
  declineGroupInvitation,
  type GroupInvitation,
  groupsErrorStatus,
  groupsErrorText,
} from "../groupsApi";
import { groupInvitationsQueryKey, useGroupInvitationsQuery } from "../hooks";
import {
  INVITATION_NO_LONGER_VALID_TEXT,
  invitationsRowLabel,
  invitationsViewState,
  invitedYouText,
  memberCountLabel,
  withoutInvitation,
} from "../invitationsState";

/**
 * "Invitations · N" at the top of Chats, opening the list of group
 * invitations. Hidden when there are none. Refreshes on app start, on window
 * focus, and whenever `refreshKey` changes (Chats opened).
 */
export function InvitationsEntry({
  onOpenGroup,
  refreshKey,
}: {
  onOpenGroup: (channelId: string) => void;
  refreshKey: unknown;
}) {
  const query = useGroupInvitationsQuery(refreshKey);
  const [open, setOpen] = React.useState(false);
  const label = invitationsRowLabel(query.data);

  return (
    <>
      {label ? (
        <button
          className="mx-3 mb-1 flex items-center gap-2 rounded-lg border border-border/70 bg-background/70 px-3 py-2 text-left text-sm font-medium text-foreground transition-colors hover:bg-muted/50"
          data-testid="chats-invitations-row"
          onClick={() => setOpen(true)}
          type="button"
        >
          <MailPlus className="h-4 w-4 shrink-0 text-primary" />
          <span className="min-w-0 flex-1 truncate">{label}</span>
          <ChevronRight className="h-4 w-4 shrink-0 text-muted-foreground" />
        </button>
      ) : null}
      {open ? (
        <InvitationsDialog
          data={query.data}
          isError={query.isError}
          isLoading={query.isLoading}
          onOpenChange={setOpen}
          onOpenGroup={(channelId) => {
            setOpen(false);
            onOpenGroup(channelId);
          }}
          onRetry={() => void query.refetch()}
        />
      ) : null}
    </>
  );
}

function InvitationsDialog({
  data,
  isError,
  isLoading,
  onOpenChange,
  onOpenGroup,
  onRetry,
}: {
  data: GroupInvitation[] | undefined;
  isError: boolean;
  isLoading: boolean;
  onOpenChange: (open: boolean) => void;
  onOpenGroup: (channelId: string) => void;
  onRetry: () => void;
}) {
  const queryClient = useQueryClient();
  const account = useIdentityQuery().data?.pubkey?.toLowerCase() ?? "";
  const listKey = groupInvitationsQueryKey(account);
  const [busy, setBusy] = React.useState<{ id: string; join: boolean } | null>(
    null,
  );
  const state = invitationsViewState({ data, isError, isLoading });
  const inviters = React.useMemo(
    () => [...new Set((data ?? []).map((invitation) => invitation.inviter))],
    [data],
  );
  const profiles = useUsersBatchQuery(inviters, {
    enabled: inviters.length > 0,
  }).data?.profiles;

  const remove = (id: string) =>
    queryClient.setQueryData<GroupInvitation[]>(listKey, (current) =>
      withoutInvitation(current, id),
    );

  async function respond(invitation: GroupInvitation, join: boolean) {
    setBusy({ id: invitation.id, join });
    try {
      if (join) {
        const result = await acceptGroupInvitation(invitation.id);
        remove(invitation.id);
        await queryClient.invalidateQueries({ queryKey: channelsQueryKey });
        onOpenGroup(result.channelId);
      } else {
        await declineGroupInvitation(invitation.id);
        remove(invitation.id);
      }
    } catch (error) {
      if (groupsErrorStatus(error) === 409) {
        remove(invitation.id);
        toast(INVITATION_NO_LONGER_VALID_TEXT);
      } else {
        toast.error(
          groupsErrorText(error) || "Something went wrong. Try again.",
        );
      }
    } finally {
      setBusy(null);
    }
  }

  return (
    <Dialog onOpenChange={onOpenChange} open>
      <DialogContent
        aria-describedby={undefined}
        className="max-w-md"
        data-testid="invitations-dialog"
      >
        <DialogHeader>
          <DialogTitle>Invitations</DialogTitle>
        </DialogHeader>
        {state.kind === "loading" ? (
          <p className="py-6 text-center text-sm text-muted-foreground">
            Loading...
          </p>
        ) : state.kind === "error" ? (
          <div className="flex flex-col items-center gap-3 py-6">
            <p className="text-sm text-muted-foreground">
              Couldn't load invitations.
            </p>
            <Button onClick={onRetry} size="sm" type="button" variant="outline">
              Try again
            </Button>
          </div>
        ) : state.kind === "empty" ? (
          <p
            className="py-6 text-center text-sm text-muted-foreground"
            data-testid="invitations-empty"
          >
            No invitations
          </p>
        ) : (
          <div className="max-h-[60vh] divide-y divide-border/55 overflow-y-auto rounded-xl border border-border/70">
            {state.invitations.map((invitation) => {
              const inviterName = resolveUserLabel({
                pubkey: invitation.inviter,
                profiles,
              });
              const joining = busy?.id === invitation.id && busy.join;
              return (
                <div
                  className="flex items-center gap-3 px-4 py-3"
                  data-testid={`invitation-${invitation.id}`}
                  key={invitation.id}
                >
                  <GroupLetterAvatar
                    channelId={invitation.channel.id}
                    isPrivate={invitation.channel.visibility === "private"}
                    label={invitation.channel.name}
                    size="md"
                  />
                  <div className="min-w-0 flex-1">
                    <p className="truncate text-sm font-medium">
                      {invitation.channel.name}
                    </p>
                    <p className="truncate text-xs text-muted-foreground">
                      {`${invitedYouText(inviterName)} · ${memberCountLabel(
                        invitation.channel.memberCount,
                      )}`}
                    </p>
                  </div>
                  <Button
                    data-testid={`invitation-decline-${invitation.id}`}
                    disabled={busy !== null}
                    onClick={() => void respond(invitation, false)}
                    size="sm"
                    type="button"
                    variant="outline"
                  >
                    Decline
                  </Button>
                  <Button
                    data-testid={`invitation-join-${invitation.id}`}
                    disabled={busy !== null}
                    onClick={() => void respond(invitation, true)}
                    size="sm"
                    type="button"
                  >
                    {joining ? "Joining..." : "Join"}
                  </Button>
                </div>
              );
            })}
          </div>
        )}
      </DialogContent>
    </Dialog>
  );
}
