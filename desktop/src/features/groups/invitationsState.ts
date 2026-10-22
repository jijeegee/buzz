import { t } from "@/shared/i18n";
import type { GroupInvitation } from "./groupsApi";

export type InvitationsViewState =
  | { kind: "loading" }
  | { kind: "error" }
  | { kind: "empty" }
  | { kind: "list"; invitations: GroupInvitation[] };

/** What the Invitations list shows for a query snapshot. */
export function invitationsViewState(query: {
  data: GroupInvitation[] | undefined;
  isError: boolean;
  isLoading: boolean;
}): InvitationsViewState {
  if (query.data && query.data.length > 0) {
    return { kind: "list", invitations: query.data };
  }
  if (query.data) return { kind: "empty" };
  if (query.isError) return { kind: "error" };
  return { kind: "loading" };
}

/** "Invitations · N" for the Chats row, or null when there are none. */
export function invitationsRowLabel(
  invitations: GroupInvitation[] | undefined,
): string | null {
  const count = invitations?.length ?? 0;
  return count > 0 ? t("groups.invitations.rowLabel", { count }) : null;
}

export function memberCountLabel(count: number): string {
  return t("groups.invitations.memberCount", { count });
}

export function invitedYouText(name: string): string {
  return t("groups.invitations.invitedYou", { name });
}

export function invitationNoLongerValidText(): string {
  return t("groups.invitations.noLongerValid");
}

/** The list after one invitation was joined, declined or found invalid. */
export function withoutInvitation(
  invitations: GroupInvitation[] | undefined,
  id: string,
): GroupInvitation[] {
  return (invitations ?? []).filter((invitation) => invitation.id !== id);
}
