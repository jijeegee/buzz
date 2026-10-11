import type { AccountSummary } from "@/shared/api/tokenAuth";

type Labelled = Pick<
  AccountSummary,
  "buzzName" | "displayName" | "email" | "pubkey"
>;

/**
 * The name an account goes by in menus: its Buzz profile name, else its
 * Google name, else its email, else a short id.
 */
export function accountLabel(account: Labelled): string {
  const name = account.buzzName?.trim() || account.displayName?.trim();
  if (name) return name;
  const email = account.email?.trim();
  if (email) return email;
  return `Account ${account.pubkey.slice(0, 8)}`;
}

/** The email under an account's name, unless the name already is the email. */
export function accountEmailDetail(account: Labelled): string | null {
  const email = account.email?.trim();
  return email && email !== accountLabel(account) ? email : null;
}

/**
 * The account a sign-out of the active one switches to: the most recently
 * used other account signed in on the same server (the backend picks the
 * same one).
 */
export function accountAfterSignOut(
  accounts: readonly AccountSummary[],
): AccountSummary | null {
  const active = accounts.find((account) => account.active);
  if (!active) return null;
  return (
    accounts.find(
      (account) =>
        !account.active &&
        account.state === "signed_in" &&
        account.origin === active.origin,
    ) ?? null
  );
}

/**
 * Accounts the profile menu lists, in the backend's most-recent order. A
 * signed-out account is left out: adding it again signs it back in.
 */
export function menuAccounts(
  accounts: readonly AccountSummary[],
): AccountSummary[] {
  return accounts.filter((account) => account.state !== "signed_out");
}

/** Accounts the menu can switch to directly (others need a fresh sign-in). */
export function canSwitchTo(account: AccountSummary): boolean {
  return (
    !account.active &&
    (account.state === "signed_in" || account.state === "key_only")
  );
}

/**
 * The agents an account sees in its Agents list: its own, plus agents whose
 * owner is unknown (never hidden, so they can still be managed). Every agent
 * shows while the active account is not known yet.
 */
export function agentsOfAccount<T extends { ownerPubkey: string | null }>(
  agents: readonly T[],
  account: string | null | undefined,
): T[] {
  if (!account) return [...agents];
  const me = account.toLowerCase();
  return agents.filter(
    (agent) =>
      agent.ownerPubkey == null || agent.ownerPubkey.toLowerCase() === me,
  );
}
