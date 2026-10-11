import type { AccountSummary } from "@/shared/api/tokenAuth";

/** The name an account goes by in menus: its name, else email, else a short id. */
export function accountLabel(
  account: Pick<AccountSummary, "displayName" | "email" | "pubkey">,
): string {
  const name = account.displayName?.trim();
  if (name) return name;
  const email = account.email?.trim();
  if (email) return email;
  return `Account ${account.pubkey.slice(0, 8)}`;
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
