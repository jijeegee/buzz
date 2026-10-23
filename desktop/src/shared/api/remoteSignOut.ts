import { invokeTauri } from "@/shared/api/tauri";

/** The relay's refusal for a session signed out (e.g. from another device). */
const SIGNED_OUT_REFUSAL = "restricted: signed_out";

/** Whether an AUTH refusal, NOTICE or close reason says "signed out". */
export function isSignedOutRefusal(message: unknown): boolean {
  return (
    typeof message === "string" &&
    message.trim().toLowerCase().startsWith(SIGNED_OUT_REFUSAL)
  );
}

/**
 * Ask the app to confirm with the server whether `account` (default: the
 * active account) was signed out remotely on `relayUrl` (default: the
 * current community); only a confirmed end signs that one account out
 * here. Fire and forget.
 */
export function checkRemoteSignOut(account?: string, relayUrl?: string): void {
  void invokeTauri<boolean>("check_remote_sign_out", {
    account: account ?? null,
    relayUrl: relayUrl ?? null,
  }).catch(() => {
    // A failed check changes nothing; the next refusal asks again.
  });
}

/** [`checkRemoteSignOut`] for the active account when `message` says "signed out". */
export function noteSignedOutRefusal(message: unknown): void {
  if (isSignedOutRefusal(message)) checkRemoteSignOut();
}
