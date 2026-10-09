import * as React from "react";
import { toast } from "sonner";

import {
  getTokenAuthStatus,
  type TokenAuthStatus,
} from "@/shared/api/tokenAuth";
import { useGoogleLogin } from "@/shared/hooks/useGoogleLogin";
import { GoogleLoginProgress } from "@/shared/ui/GoogleLoginProgress";
import { Button } from "@/shared/ui/button";
import { Spinner } from "@/shared/ui/spinner";

/**
 * Secondary Google sign-in for accounts created by an earlier Buzz version
 * (legacy token accounts). Plain Google sign-in cannot reach those accounts
 * on a new device, so the route stays, in plain words.
 */
export const OLDER_ACCOUNT_SIGN_IN_LABEL = "Sign in to an older Buzz account";
export const OLDER_ACCOUNT_SIGN_IN_TITLE =
  "Use this only if your Buzz account was created with an earlier version of Buzz.";

/**
 * "Sign in with Google" for onboarding. Rendered only when the community's
 * relay offers Google onboarding and this device is not signed in yet.
 * Native code restores the same signing key or links the existing key.
 */
export function GoogleSignInButton({
  disabled,
  allowRestore = false,
  note = "Sign in with your Google account to get started.",
  onComplete,
  showWhenSignedIn = false,
}: {
  disabled?: boolean;
  allowRestore?: boolean;
  note?: string;
  onComplete?: () => Promise<void>;
  /** Restoring a lost identity signs in again over a surviving session. */
  showWhenSignedIn?: boolean;
}) {
  const [status, setStatus] = React.useState<TokenAuthStatus | null>(null);
  const [pending, setPending] = React.useState(false);
  const login = useGoogleLogin();

  React.useEffect(() => {
    let cancelled = false;
    getTokenAuthStatus()
      .then((next) => {
        if (!cancelled) setStatus(next);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, []);

  if (!status?.supported) return null;
  if (status.state === "active" && !showWhenSignedIn) return null;

  function signIn(existingTokenAccount = false) {
    setPending(true);
    login
      .run(existingTokenAccount ? false : allowRestore, existingTokenAccount)
      .then(async (next) => {
        if (!next) return;
        if (onComplete) await onComplete();
        else window.location.reload();
      })
      .catch((error: unknown) => {
        setPending(false);
        toast.error(
          error instanceof Error
            ? error.message
            : typeof error === "string"
              ? error
              : "Google sign-in failed.",
        );
      })
      .finally(() => setPending(false));
  }

  return (
    <div className="space-y-2">
      <p className="text-sm text-muted-foreground">{note}</p>
      <Button
        className="h-10 w-full"
        data-testid="onboarding-google-sign-in"
        disabled={disabled || pending}
        onClick={() => signIn()}
        type="button"
        variant="outline"
      >
        {pending ? <Spinner aria-hidden className="h-4 w-4 border-2" /> : null}
        {pending ? "Signing in…" : "Sign in with Google"}
      </Button>
      <GoogleLoginProgress login={login} />
      {status.keyBackupSupported &&
      !status.keyBackup &&
      !status.legacyTokenAccount &&
      status.state === "signed_out" ? (
        <Button
          className="w-full"
          data-testid="existing-token-sign-in"
          disabled={disabled || pending}
          onClick={() => signIn(true)}
          type="button"
          variant="outline"
          title={OLDER_ACCOUNT_SIGN_IN_TITLE}
        >
          {OLDER_ACCOUNT_SIGN_IN_LABEL}
        </Button>
      ) : null}
    </div>
  );
}
