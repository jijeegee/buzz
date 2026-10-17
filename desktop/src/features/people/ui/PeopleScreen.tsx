import * as React from "react";
import { MessageSquare, Star, UserMinus, UserPlus, Users } from "lucide-react";
import { toast } from "sonner";

import { useBlockedPeople } from "@/features/people/BlockedPeopleContext";
import { usePeopleFavorites } from "@/features/people/PeopleFavoritesContext";
import { useMessagePerson } from "@/features/people/useMessagePerson";
import { usePeopleFriends } from "@/features/people/usePeopleFriends";
import { useUserProfileQuery } from "@/features/profile/hooks";
import { ProfileAvatar } from "@/features/profile/ui/ProfileAvatar";
import { useIdentityQuery } from "@/shared/api/hooks";
import { useHistorySearchState } from "@/shared/hooks/useHistorySearchState";
import { cn } from "@/shared/lib/cn";
import { Button } from "@/shared/ui/button";
import { PersonSafetyMenu } from "./PersonSafetyMenu";

const PEOPLE_PANEL_SEARCH_KEYS = ["profile"] as const;

/**
 * The People tab's main pane: the person picked in the left list, or a hint
 * while no one is picked. The list itself lives in the left column.
 */
export function PeopleScreen() {
  const identityQuery = useIdentityQuery();
  const currentPubkey = identityQuery.data?.pubkey;
  const { values } = useHistorySearchState(PEOPLE_PANEL_SEARCH_KEYS);
  const pubkey = values.profile?.toLowerCase() ?? null;
  return (
    <div
      className="flex min-h-0 min-w-0 flex-1 flex-col items-center overflow-y-auto px-6 py-16"
      data-testid="people-page-content"
    >
      {pubkey ? (
        <PersonProfile currentPubkey={currentPubkey} pubkey={pubkey} />
      ) : (
        <div className="m-auto flex max-w-xs flex-col items-center gap-3 text-center text-muted-foreground">
          <Users className="h-10 w-10" />
          <p className="text-sm">
            Pick someone on the left to see their profile, or find people by
            name to add them as friends.
          </p>
        </div>
      )}
    </div>
  );
}

function PersonProfile({
  currentPubkey,
  pubkey,
}: {
  currentPubkey: string | undefined;
  pubkey: string;
}) {
  const profileQuery = useUserProfileQuery(pubkey);
  const favorites = usePeopleFavorites();
  const friends = usePeopleFriends(currentPubkey);
  const { isBlocked } = useBlockedPeople();
  const onMessage = useMessagePerson();
  const [pending, setPending] = React.useState(false);
  const profile = profileQuery.data;
  const name =
    profile?.displayName?.trim() || profile?.nip05Handle?.trim() || "Unknown";
  const isFavorite = favorites?.isFavorite(pubkey) ?? false;
  const isFriend = friends.isFriend(pubkey);
  const isSelf = currentPubkey?.toLowerCase() === pubkey;
  const blocked = isBlocked(pubkey);

  const toggleFriend = () => {
    setPending(true);
    (isFriend ? friends.removeFriend(pubkey) : friends.addFriend(pubkey))
      .catch((error: unknown) =>
        toast.error(
          error instanceof Error ? error.message : "Could not update friends",
        ),
      )
      .finally(() => setPending(false));
  };

  return (
    <div
      className="flex w-full max-w-md flex-col items-center gap-3 text-center"
      data-testid={`people-profile-${pubkey}`}
    >
      <ProfileAvatar
        avatarUrl={profile?.avatarUrl ?? null}
        className="h-24 w-24 text-3xl shadow-none"
        iconClassName="h-10 w-10"
        label={name}
        shape="circle"
      />
      <div className="flex items-center gap-2">
        <h1 className="text-xl font-semibold text-foreground">{name}</h1>
        {favorites && !isSelf ? (
          <button
            aria-label={
              isFavorite ? "Remove from favorites" : "Add to favorites"
            }
            aria-pressed={isFavorite}
            className="rounded-md p-1 text-muted-foreground hover:bg-muted"
            data-testid="people-profile-favorite"
            onClick={() =>
              isFavorite
                ? favorites.removeFavorite(pubkey)
                : favorites.addFavorite(pubkey)
            }
            title={isFavorite ? "Remove from favorites" : "Add to favorites"}
            type="button"
          >
            <Star
              className={cn(
                "h-5 w-5",
                isFavorite && "fill-amber-400 text-amber-400",
              )}
            />
          </button>
        ) : null}
      </div>
      {profile?.about?.trim() ? (
        <p className="whitespace-pre-line text-sm text-muted-foreground">
          {profile.about.trim()}
        </p>
      ) : null}
      {!isSelf ? (
        <div className="mt-2 flex items-center gap-2">
          <Button
            data-testid="people-profile-message"
            onClick={() => onMessage(pubkey)}
          >
            <MessageSquare className="h-4 w-4" />
            Message
          </Button>
          <Button
            data-testid="people-profile-friend"
            disabled={pending || friends.isLoading || (blocked && !isFriend)}
            onClick={toggleFriend}
            variant={isFriend ? "ghost" : "outline"}
          >
            {isFriend ? (
              <UserMinus className="h-4 w-4" />
            ) : (
              <UserPlus className="h-4 w-4" />
            )}
            {isFriend ? "Remove friend" : "Add friend"}
          </Button>
          <PersonSafetyMenu name={name} pubkey={pubkey} />
        </div>
      ) : null}
      {!isSelf && blocked ? (
        <p
          className="text-xs text-muted-foreground"
          data-testid="people-profile-blocked"
        >
          You blocked this person.
        </p>
      ) : null}
    </div>
  );
}
