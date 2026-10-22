import { t } from "@/shared/i18n";
/**
 * How the server answered one person-add (kind:9000) to a group.
 *
 * When the person only accepts adds from friends, the relay stores an
 * invitation instead and replies `OK false "invite-sent: ..."`; an inviter
 * over the daily cap gets `rate-limited: ...`. Neither is a failure of the
 * app, so the UI shows them as an info line, not as a raw error.
 */
export type MemberAddOutcome = "invite-sent" | "rate-limited" | "error";

/** English reference text; what users see comes from `t("ui.groupAdd.rateLimited")`. */
export const INVITATIONS_RATE_LIMITED_TEXT =
  "Too many invitations. Try again later.";

export function invitationSentText(name: string): string {
  return t("ui.groupAdd.invitationSent", { name });
}

/** The relay's OK reason, without the native `relay rejected event:` prefix. */
function okReason(error: string): string {
  return error.replace(/^relay rejected event:\s*/i, "").trim();
}

export function classifyMemberAddError(error: string): MemberAddOutcome {
  const reason = okReason(error);
  if (reason.startsWith("invite-sent:")) return "invite-sent";
  if (reason.startsWith("rate-limited:")) return "rate-limited";
  return "error";
}

export type MemberAddResult = {
  added: string[];
  /** People who were sent an invitation instead of being added. */
  invited: string[];
  errors: Array<{ pubkey: string; error: string }>;
};

/**
 * Split a raw `add_channel_members` result: `invite-sent:` entries move to
 * `invited`, and `rate-limited:` entries keep their slot in `errors` with the
 * plain-language text.
 */
export function splitMemberAddResult(raw: {
  added: string[];
  errors: Array<{ pubkey: string; error: string }>;
}): MemberAddResult {
  const invited: string[] = [];
  const errors: MemberAddResult["errors"] = [];
  for (const entry of raw.errors) {
    const outcome = classifyMemberAddError(entry.error);
    if (outcome === "invite-sent") {
      invited.push(entry.pubkey);
    } else if (outcome === "rate-limited") {
      errors.push({ ...entry, error: t("ui.groupAdd.rateLimited") });
    } else {
      errors.push(entry);
    }
  }
  return { added: raw.added, invited, errors };
}
