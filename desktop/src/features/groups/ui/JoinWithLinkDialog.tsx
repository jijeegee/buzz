import * as React from "react";
import { useQueryClient } from "@tanstack/react-query";

import { channelsQueryKey } from "@/features/channels/hooks";
import { joinChannel } from "@/shared/api/tauri";
import { Button } from "@/shared/ui/button";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/shared/ui/dialog";
import { Input } from "@/shared/ui/input";
import { type GroupLinkTarget, parseGroupLink } from "../groupLink";
import {
  getGroupByHandle,
  groupsErrorStatus,
  joinGroupByInvite,
  previewGroupInvite,
} from "../groupsApi";
import { closeJoinWithLink, useJoinWithLinkState } from "../joinWithLinkStore";
import { GroupPreviewCard } from "./GroupPreviewCard";

export const JOIN_LINK_INVALID_TEXT = "This doesn't look like a group link.";
export const JOIN_LINK_NOT_FOUND_TEXT = "No group found for this link.";
const GENERIC_ERROR_TEXT = "Something went wrong. Try again.";

type FoundGroup = {
  target: GroupLinkTarget;
  id: string;
  name: string;
  description: string | null;
  memberCount: number;
  isMember: boolean;
  isPrivate: boolean;
};

async function lookUp(target: GroupLinkTarget): Promise<FoundGroup> {
  if (target.kind === "code") {
    const group = await previewGroupInvite(target.code);
    return { ...group, target, isPrivate: group.visibility === "private" };
  }
  const group = await getGroupByHandle(target.handle);
  // Only public groups have an address.
  return { ...group, target, isPrivate: group.visibility === "private" };
}

function lookupErrorText(error: unknown): string {
  return groupsErrorStatus(error) === 404
    ? JOIN_LINK_NOT_FOUND_TEXT
    : GENERIC_ERROR_TEXT;
}

function joinErrorText(error: unknown): string {
  const status = groupsErrorStatus(error);
  if (status === 404) return JOIN_LINK_NOT_FOUND_TEXT;
  if (status === 403) return "You can't join this group.";
  return GENERIC_ERROR_TEXT;
}

/**
 * ＋ › Join with link: paste a group link (or open one from a browser), see
 * the group, then Join group. Codes join through the invite route; an
 * `@address` is a public group and uses the normal public join.
 */
export function JoinWithLinkDialog({
  onOpenGroup,
}: {
  onOpenGroup: (channelId: string) => void;
}) {
  const { open, initialLink, requestId } = useJoinWithLinkState();
  const queryClient = useQueryClient();
  const [link, setLink] = React.useState("");
  const [found, setFound] = React.useState<FoundGroup | null>(null);
  const [error, setError] = React.useState<string | null>(null);
  const [joinError, setJoinError] = React.useState<string | null>(null);
  const [isLooking, setIsLooking] = React.useState(false);
  const [isJoining, setIsJoining] = React.useState(false);
  const lookupRef = React.useRef(0);

  const runLookup = React.useCallback(async (text: string) => {
    const lookupId = ++lookupRef.current;
    setFound(null);
    setJoinError(null);
    const target = parseGroupLink(text);
    if (!target) {
      setError(JOIN_LINK_INVALID_TEXT);
      return;
    }
    setError(null);
    setIsLooking(true);
    try {
      const group = await lookUp(target);
      if (lookupId === lookupRef.current) setFound(group);
    } catch (lookupError) {
      if (lookupId === lookupRef.current) setError(lookupErrorText(lookupError));
    } finally {
      if (lookupId === lookupRef.current) setIsLooking(false);
    }
  }, []);

  // Every open starts clean; a deep link fills the box and looks it up.
  // biome-ignore lint/correctness/useExhaustiveDependencies: requestId marks a new open
  React.useEffect(() => {
    if (!open) return;
    lookupRef.current += 1;
    setLink(initialLink);
    setFound(null);
    setError(null);
    setJoinError(null);
    setIsLooking(false);
    setIsJoining(false);
    if (initialLink) void runLookup(initialLink);
  }, [open, requestId]);

  function openGroup(channelId: string) {
    closeJoinWithLink();
    onOpenGroup(channelId);
  }

  async function handleJoin() {
    if (!found) return;
    setIsJoining(true);
    setJoinError(null);
    try {
      const channelId =
        found.target.kind === "code"
          ? (await joinGroupByInvite(found.target.code)).channelId
          : (await joinChannel(found.id), found.id);
      await queryClient.invalidateQueries({ queryKey: channelsQueryKey });
      openGroup(channelId);
    } catch (failure) {
      setJoinError(joinErrorText(failure));
    } finally {
      setIsJoining(false);
    }
  }

  return (
    <Dialog
      onOpenChange={(next) => {
        if (!next) closeJoinWithLink();
      }}
      open={open}
    >
      <DialogContent
        aria-describedby={undefined}
        className="max-w-md"
        data-testid="join-with-link-dialog"
      >
        <DialogHeader>
          <DialogTitle>Join with link</DialogTitle>
        </DialogHeader>
        <form
          className="flex items-center gap-2"
          onSubmit={(event) => {
            event.preventDefault();
            void runLookup(link);
          }}
        >
          <Input
            aria-invalid={error ? true : undefined}
            autoCapitalize="none"
            autoComplete="off"
            autoFocus
            className="h-9"
            data-testid="join-with-link-input"
            onChange={(event) => {
              setLink(event.target.value);
              setError(null);
            }}
            placeholder="Paste an invite link or @address"
            spellCheck={false}
            value={link}
          />
          <Button
            data-testid="join-with-link-continue"
            disabled={isLooking || link.trim().length === 0}
            type="submit"
          >
            {isLooking ? "Looking..." : "Continue"}
          </Button>
        </form>
        {error ? (
          <p
            className="text-sm text-destructive"
            data-testid="join-with-link-error"
            role="alert"
          >
            {error}
          </p>
        ) : null}
        {found ? (
          <GroupPreviewCard
            error={joinError}
            group={found}
            isJoining={isJoining}
            onJoin={() => void handleJoin()}
            onOpen={() => openGroup(found.id)}
          />
        ) : null}
      </DialogContent>
    </Dialog>
  );
}
