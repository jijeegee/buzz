import * as React from "react";
import { toast } from "sonner";

import {
  getTokenAuthStatus,
  loginWithGoogle,
  type TokenAuthStatus,
} from "@/shared/api/tokenAuth";
import { Button } from "@/shared/ui/button";
import { Spinner } from "@/shared/ui/spinner";

/**
 * "Sign in with Google" for onboarding. Rendered only when the community's
 * relay offers token sign-in and this device is not signed in yet. Success
 * reloads into the account identity (every identity-scoped store restarts).
 */
export function GoogleSignInButton({ disabled }: { disabled?: boolean }) {
  const [status, setStatus] = React.useState<TokenAuthStatus | null>(null);
  const [pending, setPending] = React.useState(false);

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

  if (!status?.supported || status.state === "active") return null;

  function signIn() {
    setPending(true);
    loginWithGoogle()
      .then(() => window.location.reload())
      .catch((error: unknown) => {
        setPending(false);
        toast.error(
          error instanceof Error
            ? error.message
            : typeof error === "string"
              ? error
              : "Google sign-in failed.",
        );
      });
  }

  return (
    <Button
      className="h-10 w-full"
      data-testid="onboarding-google-sign-in"
      disabled={disabled || pending}
      onClick={signIn}
      type="button"
      variant="outline"
    >
      {pending ? <Spinner aria-hidden className="h-4 w-4 border-2" /> : null}
      {pending ? "Finish signing in in your browser…" : "Sign in with Google"}
    </Button>
  );
}
