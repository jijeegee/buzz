import {
  BellOff,
  DoorClosed,
  Pencil,
  RotateCcw,
  Target,
  UserRoundPlus,
} from "lucide-react";
import * as React from "react";
import { toast } from "sonner";

import { useUsersBatchQuery } from "@/features/profile/hooks";
import { resolveUserLabel } from "@/features/profile/lib/identity";
import { ProfileAvatar } from "@/features/profile/ui/ProfileAvatar";
import type { Channel, ChannelMember } from "@/shared/api/types";
import { normalizePubkey } from "@/shared/lib/pubkey";
import { Button } from "@/shared/ui/button";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/shared/ui/dialog";
import { Input } from "@/shared/ui/input";
import { PanelSectionGroup } from "@/shared/ui/PanelSectionGroup";
import { Switch } from "@/shared/ui/switch";

import { useChatPrefs } from "../ChatPrefsContext";
import {
  normalizePrivateChatName,
  useChatPrivateNames,
} from "../lib/chatPrivateNames";
import { peopleCountLabel } from "../lib/groupChat";
import { useChatTitle } from "../useChatTitle";
import { ChannelChatAvatar } from "./ChannelChatAvatar";
import { ChannelGoalsSwitchRow } from "./ChannelGoalsSwitchRow";
import {
  ActionFieldRow,
  FieldGroup,
  IngressRow,
} from "./ChannelManagementSheetRows";

/** Most people listed inline; the people dialog shows everyone. */
const INLINE_PEOPLE_LIMIT = 50;

const MUTE_SWITCH_ID = "chat-info-mute-switch";

/**
 * Notification mute for any room (DMs, chats, groups). Mentions of you still
 * notify. Hidden when the shell's preference sync is unavailable.
 */
export function MuteNotificationsRow({ channelId }: { channelId: string }) {
  const prefs = useChatPrefs();
  if (!prefs) return null;
  const muted = prefs.mutedChannelIds.has(channelId);
  return (
    <PanelSectionGroup testId="channel-management-mute">
      <div className="flex min-h-16 w-full items-center gap-3 px-4 py-3">
        <BellOff className="h-4 w-4 shrink-0 text-muted-foreground" />
        <div className="min-w-0 flex-1">
          <label
            className="block text-sm font-medium text-foreground"
            htmlFor={MUTE_SWITCH_ID}
          >
            Mute notifications
          </label>
          <p className="mt-0.5 text-sm text-muted-foreground/70">
            You'll still hear about mentions of you.
          </p>
        </div>
        <Switch
          checked={muted}
          data-testid={MUTE_SWITCH_ID}
          id={MUTE_SWITCH_ID}
          onCheckedChange={(next) => {
            if (next) prefs.muteChannel(channelId);
            else prefs.unmuteChannel(channelId);
          }}
        />
      </div>
    </PanelSectionGroup>
  );
}

type GoalsSwitchProps = {
  canToggle: boolean;
  enabled: boolean;
  error: unknown;
  isSaving: boolean;
  onChange: (enabled: boolean) => void;
  onOpenGoals: () => void;
};

/**
 * "Chat info" for an ownerless chat: the people picture, title and count,
 * the people list, and only the controls every member has (add people,
 * a private name, mute, goals, leave). No owner or admin controls.
 */
export function ChatInfoSummary({
  channel,
  currentPubkey,
  goals,
  isLeaving,
  leaveError,
  memberCount,
  members,
  membersError,
  onLeave,
  onOpenPeople,
}: {
  channel: Channel;
  currentPubkey?: string;
  goals: GoalsSwitchProps;
  isLeaving: boolean;
  leaveError: unknown;
  memberCount: number;
  members: ChannelMember[];
  membersError: unknown;
  onLeave: () => void;
  /** Opens the people dialog (search everyone, add people). */
  onOpenPeople?: () => void;
}) {
  const title = useChatTitle(channel, channel.name);
  const prefs = useChatPrefs();
  const privateName = useChatPrivateNames().get(channel.id) ?? null;
  const [isRenameOpen, setIsRenameOpen] = React.useState(false);
  const isMember = members.some(
    (member) =>
      currentPubkey !== undefined &&
      normalizePubkey(member.pubkey) === normalizePubkey(currentPubkey),
  );

  return (
    <div className="space-y-6 pt-3" data-testid="chat-info-summary">
      <div
        className="flex flex-col items-center gap-3 py-3 text-center"
        data-testid="chat-info-hero"
      >
        <ChannelChatAvatar
          memberPubkeys={
            members.length > 0
              ? members.map((m) => m.pubkey)
              : channel.memberPubkeys
          }
          size="xl"
          testId="chat-info-avatar"
        />
        <div className="flex max-w-full flex-col items-center">
          <h3
            className="max-w-full truncate text-xl font-semibold tracking-tight"
            data-testid="chat-info-title"
          >
            {title}
          </h3>
          <p
            className="mt-1 text-sm text-muted-foreground/70"
            data-testid="chat-info-count"
          >
            {peopleCountLabel(memberCount)}
          </p>
        </div>
      </div>

      {membersError instanceof Error ? (
        <p className="rounded-xl border border-destructive/30 bg-destructive/10 px-3 py-2 text-sm text-destructive">
          {membersError.message}
        </p>
      ) : null}

      <ChatPeopleList
        currentPubkey={currentPubkey}
        members={members}
        onAddPeople={isMember ? onOpenPeople : undefined}
        onShowAll={onOpenPeople}
      />

      {prefs ? (
        <FieldGroup testId="chat-info-name">
          <ActionFieldRow
            description={
              privateName ? `Only you see "${privateName}"` : undefined
            }
            icon={Pencil}
            label="Rename for me"
            onClick={() => setIsRenameOpen(true)}
            testId="chat-info-rename"
          />
          {privateName ? (
            <ActionFieldRow
              icon={RotateCcw}
              label="Restore name"
              onClick={() => {
                void prefs
                  .setPrivateChatName(channel.id, null)
                  .catch((error: unknown) =>
                    toast.error(
                      error instanceof Error
                        ? error.message
                        : "Could not restore the name.",
                    ),
                  );
              }}
              testId="chat-info-restore-name"
            />
          ) : null}
        </FieldGroup>
      ) : null}

      <MuteNotificationsRow channelId={channel.id} />

      <ChannelGoalsSwitchRow
        canToggle={goals.canToggle}
        checked={goals.enabled}
        error={goals.error}
        isSaving={goals.isSaving}
        onCheckedChange={goals.onChange}
      />
      {goals.enabled ? (
        <IngressRow
          helpText="One top goal for this conversation, split into smaller goals. Agents here see it every turn."
          icon={Target}
          label="Goals"
          onClick={goals.onOpenGoals}
          testId="channel-goals-ingress"
        />
      ) : null}

      {isMember ? (
        <FieldGroup testId="chat-info-actions">
          <ActionFieldRow
            destructive
            disabled={isLeaving}
            icon={DoorClosed}
            label={isLeaving ? "Leaving chat..." : "Leave chat"}
            onClick={onLeave}
            testId="chat-info-leave"
          />
        </FieldGroup>
      ) : null}
      {leaveError instanceof Error ? (
        <p className="text-sm text-destructive">{leaveError.message}</p>
      ) : null}

      {prefs ? (
        <RenameForMeDialog
          currentName={privateName ?? ""}
          onOpenChange={setIsRenameOpen}
          onSave={(name) => prefs.setPrivateChatName(channel.id, name)}
          open={isRenameOpen}
          placeholder={title}
        />
      ) : null}
    </div>
  );
}

function ChatPeopleList({
  currentPubkey,
  members,
  onAddPeople,
  onShowAll,
}: {
  currentPubkey?: string;
  members: ChannelMember[];
  onAddPeople?: () => void;
  onShowAll?: () => void;
}) {
  const [query, setQuery] = React.useState("");
  const pubkeys = React.useMemo(
    () => members.map((member) => normalizePubkey(member.pubkey)),
    [members],
  );
  const profiles = useUsersBatchQuery(pubkeys, {
    enabled: pubkeys.length > 0,
  }).data?.profiles;
  const people = React.useMemo(
    () =>
      pubkeys.map((pubkey) => ({
        pubkey,
        avatarUrl: profiles?.[pubkey]?.avatarUrl ?? null,
        isAgent: profiles?.[pubkey]?.isAgent === true,
        label: resolveUserLabel({ currentPubkey, profiles, pubkey }),
      })),
    [currentPubkey, profiles, pubkeys],
  );
  const needle = query.trim().toLowerCase();
  const matches = needle
    ? people.filter((person) => person.label.toLowerCase().includes(needle))
    : people;
  const shown = matches.slice(0, INLINE_PEOPLE_LIMIT);

  return (
    <FieldGroup testId="chat-info-people" title="People">
      {onAddPeople ? (
        <ActionFieldRow
          icon={UserRoundPlus}
          label="Add people"
          onClick={onAddPeople}
          testId="chat-info-add-people"
        />
      ) : null}
      {members.length > 5 ? (
        <div className="px-4 py-2">
          <Input
            aria-label="Search people"
            data-testid="chat-info-people-search"
            onChange={(event) => setQuery(event.target.value)}
            placeholder="Search people"
            value={query}
          />
        </div>
      ) : null}
      {shown.map((person) => (
        <div
          className="flex min-h-12 items-center gap-3 px-4 py-2"
          data-testid="chat-info-person"
          key={person.pubkey}
        >
          <ProfileAvatar
            avatarUrl={person.avatarUrl}
            className="size-7 text-xs"
            label={person.label}
            mediaVariant="small"
            shape={person.isAgent ? "squircle" : "circle"}
          />
          <span className="min-w-0 flex-1 truncate text-sm">
            {person.label}
          </span>
        </div>
      ))}
      {needle && matches.length === 0 ? (
        <p className="px-4 py-3 text-sm text-muted-foreground">
          No one matches.
        </p>
      ) : null}
      {matches.length > shown.length && onShowAll ? (
        <button
          className="w-full px-4 py-3 text-left text-sm text-muted-foreground hover:bg-muted/40"
          data-testid="chat-info-people-all"
          onClick={onShowAll}
          type="button"
        >
          See all {peopleCountLabel(matches.length)}
        </button>
      ) : null}
    </FieldGroup>
  );
}

function RenameForMeDialog({
  currentName,
  onOpenChange,
  onSave,
  open,
  placeholder,
}: {
  currentName: string;
  onOpenChange: (open: boolean) => void;
  onSave: (name: string | null) => Promise<void>;
  open: boolean;
  placeholder: string;
}) {
  const [draft, setDraft] = React.useState(currentName);
  const [isSaving, setIsSaving] = React.useState(false);
  const [error, setError] = React.useState<string | null>(null);

  React.useEffect(() => {
    if (open) {
      setDraft(currentName);
      setError(null);
    }
  }, [currentName, open]);

  async function save() {
    const name = normalizePrivateChatName(draft);
    setIsSaving(true);
    setError(null);
    try {
      await onSave(name || null);
      onOpenChange(false);
    } catch (cause) {
      setError(
        cause instanceof Error ? cause.message : "Could not save the name.",
      );
    } finally {
      setIsSaving(false);
    }
  }

  return (
    <Dialog onOpenChange={onOpenChange} open={open}>
      <DialogContent aria-describedby={undefined} className="max-w-sm">
        <DialogHeader>
          <DialogTitle>Rename for me</DialogTitle>
        </DialogHeader>
        <p className="text-sm text-muted-foreground">Only you see this name.</p>
        <Input
          aria-label="Chat name"
          autoFocus
          data-testid="chat-info-rename-input"
          disabled={isSaving}
          onChange={(event) => setDraft(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault();
              void save();
            }
          }}
          placeholder={placeholder}
          value={draft}
        />
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
            data-testid="chat-info-rename-save"
            disabled={isSaving}
            onClick={() => void save()}
            size="sm"
            type="button"
          >
            {isSaving ? "Saving..." : "Save"}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
