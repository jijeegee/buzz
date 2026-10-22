import { GroupLetterAvatar } from "@/features/home/ui/ConversationLetterAvatar";
import { useT } from "@/shared/i18n";
import { Button } from "@/shared/ui/button";
import { memberCountLabel } from "../invitationsState";

/**
 * A group before joining: avatar, name, description and member count, with
 * a full-width Join group button. Private groups show name + members only.
 */
export function GroupPreviewCard({
  error,
  group,
  isJoining,
  onJoin,
  onOpen,
}: {
  error?: string | null;
  group: {
    id: string;
    name: string;
    description: string | null;
    memberCount: number;
    isMember: boolean;
    isPrivate: boolean;
    picture?: string | null;
  };
  isJoining: boolean;
  onJoin: () => void;
  onOpen: () => void;
}) {
  const t = useT();
  const description = group.isPrivate ? null : group.description?.trim();
  return (
    <div
      className="flex flex-col items-center rounded-xl border border-border/70 bg-background/70 px-5 py-6 text-center"
      data-testid="group-preview-card"
    >
      <GroupLetterAvatar
        channelId={group.id}
        isPrivate={group.isPrivate}
        label={group.name}
        picture={group.picture}
        size="xl"
      />
      <p
        className="mt-4 max-w-full truncate text-lg font-semibold tracking-tight"
        data-testid="group-preview-name"
      >
        {group.name}
      </p>
      {description ? (
        <p className="mt-1 line-clamp-3 max-w-full text-sm text-muted-foreground">
          {description}
        </p>
      ) : null}
      <p
        className="mt-1 text-sm text-muted-foreground"
        data-testid="group-preview-members"
      >
        {memberCountLabel(group.memberCount)}
      </p>
      {group.isMember ? (
        <Button
          className="mt-5 w-full"
          data-testid="group-preview-open"
          onClick={onOpen}
          type="button"
        >
          {t("groups.preview.open")}
        </Button>
      ) : (
        <Button
          className="mt-5 w-full"
          data-testid="group-preview-join"
          disabled={isJoining}
          onClick={onJoin}
          type="button"
        >
          {isJoining ? t("groups.preview.joining") : t("groups.preview.join")}
        </Button>
      )}
      {error ? (
        <p className="mt-3 text-sm text-destructive" role="alert">
          {error}
        </p>
      ) : null}
    </div>
  );
}
