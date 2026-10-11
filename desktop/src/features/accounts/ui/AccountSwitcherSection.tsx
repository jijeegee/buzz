import { Check, Plus } from "lucide-react";

import type { WatchedUnread } from "@/features/accounts/inactiveAccountWatch/store";
import { watchKey } from "@/features/accounts/inactiveAccountWatch/store";
import { ProfileAvatar } from "@/features/profile/ui/ProfileAvatar";
import type { AccountSummary } from "@/shared/api/tokenAuth";
import { cn } from "@/shared/lib/cn";
import {
  accountEmailDetail,
  accountLabel,
  canSwitchTo,
} from "../lib/accountLabels";

const MAX_BADGE = 99;

const ROW_CLASS =
  "flex min-h-9 w-full cursor-pointer items-center gap-2 rounded-lg px-3 py-1.5 text-left text-sm text-popover-foreground outline-hidden transition-colors hover:bg-muted/50 focus:outline-none focus-visible:bg-muted/50 focus-visible:outline-none disabled:cursor-default disabled:opacity-60";

/**
 * Profile menu › accounts on this computer, most recently used first: the
 * active one is checked, another signed-in one switches, one whose sign-in
 * ended signs in again, and "Add account" signs in another Google account.
 * Other accounts show their unread dot or mention count (CommunityRail style).
 */
export function AccountSwitcherSection({
  accounts,
  busy,
  onSwitch,
  onSignIn,
  unread,
}: {
  accounts: readonly AccountSummary[];
  busy: boolean;
  onSwitch: (account: AccountSummary) => void;
  onSignIn: () => void;
  /** Watched accounts' unread state, keyed `origin|pubkey`. */
  unread?: ReadonlyMap<string, WatchedUnread>;
}) {
  return (
    <fieldset
      aria-label="Accounts"
      className="m-0 min-w-0 border-0 p-0"
      data-testid="account-switcher"
    >
      {accounts.map((account) => {
        const label = accountLabel(account);
        const needsSignIn = account.state === "needs_login";
        const detail = needsSignIn
          ? "Sign in again"
          : accountEmailDetail(account);
        const watched = account.active
          ? undefined
          : unread?.get(watchKey(account.origin, account.pubkey));
        const mentions = watched?.mentionCount ?? 0;
        return (
          <button
            aria-current={account.active ? "true" : undefined}
            className={ROW_CLASS}
            data-testid={`account-row-${account.pubkey}`}
            disabled={
              busy || (!account.active && !needsSignIn && !canSwitchTo(account))
            }
            key={account.pubkey}
            onClick={() => {
              if (account.active && !needsSignIn) return;
              if (needsSignIn) onSignIn();
              else onSwitch(account);
            }}
            role="menuitem"
            type="button"
          >
            <ProfileAvatar
              avatarUrl={account.avatarUrl}
              className="h-6 w-6 shrink-0 text-2xs"
              iconClassName="h-3 w-3"
              label={label}
            />
            <span className="min-w-0 flex-1">
              <span className="block truncate">{label}</span>
              {detail ? (
                <span
                  className={cn(
                    "block truncate text-xs",
                    needsSignIn ? "text-destructive" : "text-muted-foreground",
                  )}
                >
                  {detail}
                </span>
              ) : null}
            </span>
            {mentions > 0 ? (
              <span
                aria-label={`${mentions} mention${mentions === 1 ? "" : "s"}`}
                className="flex h-4 min-w-4 shrink-0 items-center justify-center rounded-full bg-primary px-1 text-2xs font-semibold text-primary-foreground"
                data-testid={`account-row-mentions-${account.pubkey}`}
                role="img"
              >
                {mentions > MAX_BADGE ? `${MAX_BADGE}+` : mentions}
              </span>
            ) : watched?.hasUnread ? (
              <span
                aria-label="Unread"
                className="h-2 w-2 shrink-0 rounded-full bg-primary"
                data-testid={`account-row-unread-${account.pubkey}`}
                role="img"
              />
            ) : null}
            {account.active ? (
              <Check
                aria-label="Current account"
                className="h-4 w-4 shrink-0 text-primary"
                role="img"
              />
            ) : null}
          </button>
        );
      })}
      <button
        className={ROW_CLASS}
        data-testid="account-add"
        disabled={busy}
        onClick={onSignIn}
        role="menuitem"
        type="button"
      >
        <span className="flex h-6 w-6 shrink-0 items-center justify-center">
          <Plus aria-hidden="true" className="h-4 w-4 text-muted-foreground" />
        </span>
        <span className="flex-1">Add account</span>
      </button>
    </fieldset>
  );
}
