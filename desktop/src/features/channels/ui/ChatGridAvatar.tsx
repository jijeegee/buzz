import { ProfileAvatar } from "@/features/profile/ui/ProfileAvatar";
import { cn } from "@/shared/lib/cn";

import { chatGridTiles } from "../lib/chatGridLayout";

export type ChatGridPerson = {
  avatarUrl: string | null;
  label: string;
  pubkey: string;
  isAgent?: boolean;
};

const BOX: Record<ChatGridAvatarSize, string> = {
  xs: "size-4",
  sm: "size-5",
  md: "size-7",
  lg: "size-9",
  xl: "size-20",
};

export type ChatGridAvatarSize = "xs" | "sm" | "md" | "lg" | "xl";

/**
 * A chat's picture drawn from its people (no stored photo, no lock): one
 * person fills the circle, two sit on a diagonal, three are one on top and
 * two below, four or more make a 2x2 grid of the first four.
 */
export function ChatGridAvatar({
  className,
  people,
  size = "lg",
  testId,
}: {
  className?: string;
  /** Other members first; pass yourself when you are alone. */
  people: readonly ChatGridPerson[];
  size?: ChatGridAvatarSize;
  testId?: string;
}) {
  const tiles = chatGridTiles(people.length);
  return (
    <span
      aria-hidden="true"
      className={cn("relative inline-block shrink-0", BOX[size], className)}
      data-chat-grid-avatar={tiles.length}
      data-testid={testId}
    >
      {tiles.map((tile, index) => {
        const person = people[index];
        if (!person) return null;
        return (
          <span
            className="absolute overflow-hidden rounded-full ring-1 ring-background"
            key={person.pubkey}
            style={{
              left: `${tile.left}%`,
              top: `${tile.top}%`,
              width: `${tile.size}%`,
              height: `${tile.size}%`,
            }}
          >
            <ProfileAvatar
              avatarUrl={person.avatarUrl}
              className="size-full text-3xs"
              iconClassName="size-1/2"
              label={person.label}
              mediaVariant="small"
              shape={person.isAgent ? "squircle" : "circle"}
            />
          </span>
        );
      })}
    </span>
  );
}
