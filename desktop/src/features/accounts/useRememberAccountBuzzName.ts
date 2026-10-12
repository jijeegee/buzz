import * as React from "react";

import { rememberAccountBuzzName } from "@/shared/api/tokenAuth";

/**
 * Keeps the active account's Buzz profile name in the account list, so the
 * account menu shows it for this account even while another one is active.
 */
export function useRememberAccountBuzzName(
  pubkey: string | null,
  name: string | null | undefined,
) {
  const trimmed = name?.trim() ?? "";
  React.useEffect(() => {
    if (!pubkey || !trimmed) return;
    rememberAccountBuzzName(pubkey, trimmed).catch((error) => {
      console.error("[accounts] could not record the Buzz name:", error);
    });
  }, [pubkey, trimmed]);
}
