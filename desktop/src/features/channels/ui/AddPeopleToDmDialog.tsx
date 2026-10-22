import * as React from "react";
import { toast } from "sonner";

import { useAppNavigation } from "@/app/navigation/useAppNavigation";
import {
  useCreateGroupChatMutation,
  useUpsertCachedChannel,
} from "@/features/channels/hooks";
import { NewMessageResultRow } from "@/features/messages/ui/NewMessageResultRow";
import {
  formatRecipientName,
  useNewMessageRecipients,
} from "@/features/messages/ui/useNewMessageRecipients";
import { SelectedRecipientChip } from "@/features/profile/ui/SelectedRecipientChip";
import type { Channel } from "@/shared/api/types";
import { normalizePubkey } from "@/shared/lib/pubkey";
import { Button } from "@/shared/ui/button";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/shared/ui/dialog";
import {
  MODAL_SEARCH_INPUT_CLASS,
  MODAL_SEARCH_SHELL_CLASS,
} from "@/shared/ui/modalSearchStyles";

import {
  invitationsSentText,
  MAX_NEW_CHAT_PEOPLE,
  otherChatPeople,
} from "../lib/groupChat";

/** People for the new chat: the 1:1 partner(s) first, then the picks. */
export function newChatPeopleFromDm(
  participantPubkeys: readonly string[],
  pickedPubkeys: readonly string[],
  currentPubkey: string | undefined,
): string[] {
  return otherChatPeople(
    [...participantPubkeys, ...pickedPubkeys],
    currentPubkey,
  );
}

/**
 * "Add people" from a 1:1: the 1:1 never grows, so this starts a new chat
 * with the other person plus everyone picked, then opens it.
 */
export function AddPeopleToDmDialog({
  channel,
  currentPubkey,
  onOpenChange,
  open,
}: {
  channel: Channel;
  currentPubkey: string | undefined;
  onOpenChange: (open: boolean) => void;
  open: boolean;
}) {
  const { goChannel } = useAppNavigation();
  const createGroupChat = useCreateGroupChatMutation();
  const upsertCachedChannel = useUpsertCachedChannel();
  const recipients = useNewMessageRecipients({ active: open, currentPubkey });
  const { reset } = recipients;
  const [error, setError] = React.useState<string | null>(null);
  const partners = React.useMemo(
    () => new Set(otherChatPeople(channel.participantPubkeys, currentPubkey)),
    [channel.participantPubkeys, currentPubkey],
  );

  React.useEffect(() => {
    if (!open) {
      reset();
      setError(null);
    }
  }, [open, reset]);

  const picks = recipients.selectedUsers.filter(
    (user) => !partners.has(normalizePubkey(user.pubkey)),
  );
  const people = newChatPeopleFromDm(
    channel.participantPubkeys,
    picks.map((user) => user.pubkey),
    currentPubkey,
  );
  const tooMany = people.length > MAX_NEW_CHAT_PEOPLE;

  async function start() {
    setError(null);
    try {
      const result = await createGroupChat.mutateAsync({ pubkeys: people });
      await upsertCachedChannel(result.channel);
      onOpenChange(false);
      await goChannel(result.channel.id);
      const invited = invitationsSentText(result.invited.length, people.length);
      if (invited) toast(invited);
    } catch (cause) {
      setError(
        cause instanceof Error ? cause.message : "Could not start the chat.",
      );
    }
  }

  const results = recipients.searchResults.filter(
    (user) => !partners.has(normalizePubkey(user.pubkey)),
  );

  return (
    <Dialog onOpenChange={onOpenChange} open={open}>
      <DialogContent
        aria-describedby={undefined}
        className="max-w-xl"
        data-testid="add-people-dialog"
      >
        <DialogHeader>
          <DialogTitle>Add people</DialogTitle>
        </DialogHeader>
        <p className="text-sm text-muted-foreground">
          This starts a new chat with everyone. This conversation stays as it
          is.
        </p>
        {picks.length > 0 ? (
          <div className="flex flex-wrap gap-1.5">
            {picks.map((user) => (
              <SelectedRecipientChip
                disabled={createGroupChat.isPending}
                key={user.pubkey}
                label={formatRecipientName(user)}
                onRemove={() => recipients.removeUser(user.pubkey)}
                user={user}
              />
            ))}
          </div>
        ) : null}
        <label className={MODAL_SEARCH_SHELL_CLASS} htmlFor="add-people-search">
          <input
            autoComplete="off"
            className={MODAL_SEARCH_INPUT_CLASS}
            data-testid="add-people-search"
            disabled={createGroupChat.isPending}
            id="add-people-search"
            onChange={(event) => recipients.setSearchQuery(event.target.value)}
            placeholder="Search people and agents"
            spellCheck={false}
            type="text"
            value={recipients.searchQuery}
          />
        </label>
        <div
          className="max-h-72 overflow-y-auto rounded-xl border border-border/70"
          onScroll={recipients.handleDirectoryScroll}
        >
          {results.length > 0 ? (
            results.map((user) => (
              <NewMessageResultRow
                currentPubkey={currentPubkey}
                disabled={
                  createGroupChat.isPending ||
                  (recipients.hasReachedRecipientLimit &&
                    !picks.some((pick) => pick.pubkey === user.pubkey))
                }
                isAlreadySelected={picks.some(
                  (pick) => pick.pubkey === user.pubkey,
                )}
                key={user.pubkey}
                onSelect={recipients.selectUser}
                ownerProfiles={recipients.ownerProfiles}
                user={user}
              />
            ))
          ) : (
            <p className="px-4 py-3 text-sm text-muted-foreground">
              {recipients.isDirectoryLoading
                ? "Loading..."
                : "No matching people."}
            </p>
          )}
        </div>
        {tooMany ? (
          <p className="text-sm text-muted-foreground">
            A new chat can start with up to {MAX_NEW_CHAT_PEOPLE} people.
          </p>
        ) : null}
        {error ? <p className="text-sm text-destructive">{error}</p> : null}
        <div className="flex justify-end gap-2">
          <Button
            onClick={() => onOpenChange(false)}
            size="sm"
            type="button"
            variant="outline"
          >
            Cancel
          </Button>
          <Button
            data-testid="add-people-start"
            disabled={
              picks.length === 0 || tooMany || createGroupChat.isPending
            }
            onClick={() => void start()}
            size="sm"
            type="button"
          >
            {createGroupChat.isPending ? "Starting..." : "Start chat"}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
