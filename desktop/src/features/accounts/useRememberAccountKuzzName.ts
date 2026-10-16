import * as React from "react";

import { rememberAccountKuzzName } from "@/shared/api/tokenAuth";

/**
 * Keeps the active account's Kuzz profile name in the account list, so the
 * account menu shows it for this account even while another one is active.
 */
export function useRememberAccountKuzzName(
  pubkey: string | null,
  name: string | null | undefined,
) {
  const trimmed = name?.trim() ?? "";
  React.useEffect(() => {
    if (!pubkey || !trimmed) return;
    rememberAccountKuzzName(pubkey, trimmed).catch((error) => {
      console.error("[accounts] could not record the Kuzz name:", error);
    });
  }, [pubkey, trimmed]);
}
