import type * as React from "react";

import { useSystemColorScheme } from "@/shared/theme/useSystemColorScheme";
import { Button } from "@/shared/ui/button";
import { StartupWindowDragRegion } from "@/shared/ui/StartupWindowDragRegion";
import { GoogleSignInButton } from "./GoogleSignInButton";

/**
 * Shown instead of the app while this computer is signed out of the active
 * community. Nothing was deleted; signing in with Google gets back in. The
 * signed-out account's agents stopped with it, so none are reported here.
 */
export function SignedOutScreen() {
  const systemColorScheme = useSystemColorScheme();

  return (
    <div
      className="buzz-onboarding-neutral-theme buzz-startup-shell flex items-center justify-center bg-background px-4 py-8 text-foreground"
      data-system-color-scheme={systemColorScheme}
      data-testid="signed-out-screen"
    >
      <StartupWindowDragRegion />
      <main
        aria-labelledby="signed-out-title"
        className="relative flex w-full max-w-sm flex-col items-center text-center"
      >
        <h1
          className="text-3xl font-semibold tracking-tight"
          id="signed-out-title"
        >
          You're signed out
        </h1>
        <p className="mt-3 text-sm leading-6 text-muted-foreground">
          Sign in with Google to get back to your conversations. Nothing was
          deleted.
        </p>
        <div className="mt-8 w-full">
          <GoogleSignInButton
            allowRestore
            note="Use the Google account you signed in with before."
            showWhenSignedIn
            unavailable={
              <div className="space-y-3">
                <p className="text-sm text-muted-foreground" role="status">
                  Google sign-in isn't available right now.
                </p>
                <Button
                  className="h-10 w-full"
                  onClick={() => window.location.reload()}
                  type="button"
                  variant="outline"
                >
                  Try again
                </Button>
              </div>
            }
          />
        </div>
      </main>
    </div>
  );
}

/**
 * The community app's mount boundary: while this computer is signed out, the
 * chat shell (and every relay connection it would open) never mounts.
 */
export function SignedOutGate({
  signedOut,
  children,
}: {
  signedOut: boolean;
  children: React.ReactNode;
}) {
  return signedOut ? <SignedOutScreen /> : children;
}
