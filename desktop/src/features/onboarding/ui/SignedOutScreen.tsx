import * as React from "react";

import {
  agentCountPhrase,
  listRunningLocalAgents,
} from "@/features/agents/lib/runningLocalAgents";
import { useSystemColorScheme } from "@/shared/theme/useSystemColorScheme";
import { Button } from "@/shared/ui/button";
import { StartupWindowDragRegion } from "@/shared/ui/StartupWindowDragRegion";
import { GoogleSignInButton } from "./GoogleSignInButton";

/**
 * Shown instead of the app while this computer is signed out of the active
 * community. Nothing was deleted; the only way back is the same Google
 * account (native code refuses a different one and keeps this screen).
 * Running local agents are only reported, never controlled, from here.
 */
export function SignedOutScreen() {
  const systemColorScheme = useSystemColorScheme();
  const [runningAgents, setRunningAgents] = React.useState(0);

  React.useEffect(() => {
    let cancelled = false;
    void listRunningLocalAgents().then((running) => {
      if (!cancelled) setRunningAgents(running.pubkeys.length);
    });
    return () => {
      cancelled = true;
    };
  }, []);

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
        {runningAgents > 0 ? (
          <p
            className="mt-6 text-sm text-muted-foreground"
            data-testid="signed-out-running-agents"
          >
            {agentCountPhrase(runningAgents)} still running on this computer.
          </p>
        ) : null}
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
