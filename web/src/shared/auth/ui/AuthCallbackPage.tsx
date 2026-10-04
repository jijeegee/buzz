import * as React from "react";

import { takePendingLogin } from "@/shared/auth/pkce";
import { webSession } from "@/shared/auth/session";
import { Button } from "@/shared/ui/button";

const ERROR_MESSAGES: Record<string, string> = {
  access_denied: "Google sign-in was cancelled.",
  account_disabled: "This account is disabled.",
};

/**
 * `/auth/cb`: the relay redirects here with `code` + `state` (or `error`).
 * The pending login is consumed whatever happens, so a code can be
 * completed at most once and only by the tab that started it.
 */
export function AuthCallbackPage() {
  const [error, setError] = React.useState<string | null>(null);
  const started = React.useRef(false);

  React.useEffect(() => {
    if (started.current) return;
    started.current = true;
    const params = new URLSearchParams(window.location.search);
    void (async () => {
      try {
        const pending = takePendingLogin(
          window.sessionStorage,
          params.get("state"),
        );
        const failure = params.get("error");
        if (failure) {
          throw new Error(
            ERROR_MESSAGES[failure] ?? "Sign-in failed. Try again.",
          );
        }
        const code = params.get("code");
        if (!code) throw new Error("Sign-in failed. Try again.");
        await webSession.completeLogin(code, pending.verifier);
        window.location.replace(pending.returnTo);
      } catch (cause) {
        setError(cause instanceof Error ? cause.message : "Sign-in failed.");
      }
    })();
  }, []);

  return (
    <div className="flex flex-1 flex-col items-center justify-center gap-4 px-4 py-16 text-center">
      {error ? (
        <>
          <p className="text-sm text-red-700" role="alert">
            {error}
          </p>
          <Button asChild variant="outline">
            <a href="/">Back to Buzz</a>
          </Button>
        </>
      ) : (
        <p className="text-sm text-muted-foreground" role="status">
          Signing you in…
        </p>
      )}
    </div>
  );
}
