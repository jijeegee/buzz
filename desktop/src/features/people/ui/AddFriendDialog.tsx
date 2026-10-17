import * as React from "react";
import { Check, UserPlus } from "lucide-react";
import { toast } from "sonner";

import { ProfileAvatar } from "@/features/profile/ui/ProfileAvatar";
import { lookupPerson, type PersonInfo } from "@/shared/api/people";
import { Button } from "@/shared/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/shared/ui/dialog";
import { Input } from "@/shared/ui/input";
import { Spinner } from "@/shared/ui/spinner";

export const NO_ONE_FOUND = "No one found with that ID or email";

type LookupState =
  | { status: "idle" }
  | { status: "searching" }
  | { status: "found"; person: PersonInfo }
  | { status: "not-found" }
  | { status: "error"; message: string };

function errorText(error: unknown): string {
  if (error instanceof Error && error.message) return error.message;
  if (typeof error === "string" && error) return error;
  return "Couldn't search. Try again.";
}

/**
 * "Add friend": type someone's exact Kuzz ID or email; the server answers
 * with that one person (or no one). No partial matches, so nobody's email
 * can be guessed from a list.
 */
export function AddFriendDialog({
  currentPubkey,
  isBlocked,
  isFriend,
  onAddFriend,
  onOpenChange,
  open,
}: {
  currentPubkey: string | undefined;
  isBlocked: (pubkey: string) => boolean;
  isFriend: (pubkey: string) => boolean;
  onAddFriend: (pubkey: string) => Promise<unknown>;
  onOpenChange: (open: boolean) => void;
  open: boolean;
}) {
  const [query, setQuery] = React.useState("");
  const [lookup, setLookup] = React.useState<LookupState>({ status: "idle" });
  const [adding, setAdding] = React.useState(false);
  const generation = React.useRef(0);

  React.useEffect(() => {
    if (open) {
      setQuery("");
      setLookup({ status: "idle" });
      setAdding(false);
    }
    generation.current += 1;
  }, [open]);

  const search = (event?: React.FormEvent) => {
    event?.preventDefault();
    const text = query.trim();
    if (!text) return;
    const current = ++generation.current;
    setLookup({ status: "searching" });
    lookupPerson(text)
      .then((person) => {
        if (current !== generation.current) return;
        setLookup(
          person ? { status: "found", person } : { status: "not-found" },
        );
      })
      .catch((error: unknown) => {
        if (current !== generation.current) return;
        setLookup({ status: "error", message: errorText(error) });
      });
  };

  const add = (person: PersonInfo) => {
    setAdding(true);
    onAddFriend(person.pubkey.toLowerCase())
      .then(() => {
        toast.success(`${personLabel(person)} is now your friend`);
        onOpenChange(false);
      })
      .catch((error: unknown) =>
        toast.error(
          error instanceof Error ? error.message : "Could not add friend",
        ),
      )
      .finally(() => setAdding(false));
  };

  return (
    <Dialog onOpenChange={onOpenChange} open={open}>
      <DialogContent
        className="sm:max-w-[420px]"
        data-testid="add-friend-dialog"
      >
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <UserPlus className="h-4 w-4" />
            Add friend
          </DialogTitle>
          <DialogDescription>
            Enter their Kuzz ID or email address exactly.
          </DialogDescription>
        </DialogHeader>
        <form className="flex items-center gap-2" onSubmit={search}>
          <Input
            aria-label="Kuzz ID or email"
            autoFocus
            data-testid="add-friend-query"
            onChange={(event) => {
              setQuery(event.target.value);
              if (lookup.status !== "searching") setLookup({ status: "idle" });
            }}
            placeholder="Kuzz ID or email"
            value={query}
          />
          <Button
            data-testid="add-friend-search"
            disabled={!query.trim() || lookup.status === "searching"}
            type="submit"
          >
            Find
          </Button>
        </form>
        <div aria-live="polite" className="min-h-14">
          {lookup.status === "searching" ? (
            <div className="flex items-center gap-2 px-1 text-sm text-muted-foreground">
              <Spinner aria-hidden className="h-4 w-4 border-2" />
              Searching…
            </div>
          ) : lookup.status === "not-found" ? (
            <p
              className="px-1 text-sm text-muted-foreground"
              data-testid="add-friend-not-found"
            >
              {NO_ONE_FOUND}
            </p>
          ) : lookup.status === "error" ? (
            <p className="px-1 text-sm text-destructive" role="alert">
              {lookup.message}
            </p>
          ) : lookup.status === "found" ? (
            <FoundPerson
              adding={adding}
              blocked={isBlocked(lookup.person.pubkey.toLowerCase())}
              friend={isFriend(lookup.person.pubkey.toLowerCase())}
              onAdd={() => add(lookup.person)}
              person={lookup.person}
              self={
                currentPubkey?.toLowerCase() ===
                lookup.person.pubkey.toLowerCase()
              }
            />
          ) : null}
        </div>
      </DialogContent>
    </Dialog>
  );
}

function personLabel(person: PersonInfo): string {
  return person.displayName?.trim() || person.username || "Someone";
}

function FoundPerson({
  adding,
  blocked,
  friend,
  onAdd,
  person,
  self,
}: {
  adding: boolean;
  blocked: boolean;
  friend: boolean;
  onAdd: () => void;
  person: PersonInfo;
  self: boolean;
}) {
  const name = personLabel(person);
  return (
    <div
      className="flex items-center gap-3 rounded-lg border border-border/70 px-3 py-2"
      data-testid="add-friend-result"
    >
      <ProfileAvatar
        avatarUrl={person.avatarUrl}
        className="h-9 w-9 text-xs shadow-none"
        iconClassName="h-4 w-4"
        label={name}
        shape="circle"
      />
      <div className="min-w-0 flex-1">
        <p className="truncate text-sm font-medium">{name}</p>
        {person.username ? (
          <p className="truncate text-xs text-muted-foreground">
            @{person.username}
          </p>
        ) : null}
      </div>
      {self ? (
        <span className="text-xs text-muted-foreground">This is you</span>
      ) : blocked ? (
        <span className="text-xs text-muted-foreground">Blocked</span>
      ) : friend ? (
        <span className="flex items-center gap-1 text-xs text-muted-foreground">
          <Check className="h-3.5 w-3.5" />
          Friend
        </span>
      ) : (
        <Button
          data-testid="add-friend-add"
          disabled={adding}
          onClick={onAdd}
          size="sm"
        >
          Add
        </Button>
      )}
    </div>
  );
}
