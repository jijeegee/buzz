import * as React from "react";
import { toast } from "sonner";

import {
  getTokenAuthStatus,
  identityReload,
  type TokenAuthStatus,
} from "@/shared/api/tokenAuth";
import { useGoogleLogin } from "@/shared/hooks/useGoogleLogin";
import { useT } from "@/shared/i18n";
import { GoogleLoginProgress } from "@/shared/ui/GoogleLoginProgress";
import { Button } from "@/shared/ui/button";
import { Spinner } from "@/shared/ui/spinner";
import { TermsAgreementLine } from "./TermsAgreementLine";

/**
 * "Sign in with Google" for onboarding. Rendered only when the community's
 * relay offers Google onboarding and this device is not signed in yet.
 * Native code restores the same signing key or links the existing key.
 */
export function GoogleSignInButton({
  disabled,
  allowRestore = false,
  note,
  onComplete,
  showWhenSignedIn = false,
  unavailable = null,
}: {
  disabled?: boolean;
  allowRestore?: boolean;
  note?: string;
  onComplete?: () => Promise<void>;
  /** Restoring a lost identity signs in again over a surviving session. */
  showWhenSignedIn?: boolean;
  /** Shown instead of nothing once the relay turns out not to offer Google. */
  unavailable?: React.ReactNode;
}) {
  const t = useT();
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

  if (!status) return null;
  if (!status.supported) return <>{unavailable}</>;
  if (status.state === "active" && !showWhenSignedIn) return null;

  function signIn() {
    setPending(true);
    login
      .run(allowRestore)
      .then(async (next) => {
        if (!next) return;
        if (onComplete) await onComplete();
        else identityReload.reload();
      })
      .catch((error: unknown) => {
        setPending(false);
        toast.error(
          error instanceof Error
            ? error.message
            : typeof error === "string"
              ? error
              : t("onboarding.googleSignIn.failed"),
        );
      })
      .finally(() => setPending(false));
  }

  return (
    <div className="space-y-2">
      <p className="text-sm text-muted-foreground">
        {note ?? t("onboarding.googleSignIn.note")}
      </p>
      <Button
        className="h-10 w-full"
        data-testid="onboarding-google-sign-in"
        disabled={disabled || pending}
        onClick={() => signIn()}
        type="button"
        variant="outline"
      >
        {pending ? <Spinner aria-hidden className="h-4 w-4 border-2" /> : null}
        {pending
          ? t("onboarding.googleSignIn.signingIn")
          : t("onboarding.googleSignIn.button")}
      </Button>
      <GoogleLoginProgress login={login} />
      <TermsAgreementLine />
    </div>
  );
}
