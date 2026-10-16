import * as React from "react";

import {
  signOutActiveAccountOnThisDevice,
  useHasSignedInSession,
} from "@/features/accounts/lib/signOutActiveAccount";
import { useSystemColorScheme } from "@/shared/theme/useSystemColorScheme";
import { Button } from "@/shared/ui/button";
import { StartupWindowDragRegion } from "@/shared/ui/StartupWindowDragRegion";

type CommunityApplyErrorScreenProps = {
  error: string;
  /**
   * Omitted with a built-in server: changing server cannot help, so the
   * screen offers a sign-out on this computer instead (when signed in).
   */
  onChangeCommunity?: () => void;
  onRetry: () => void;
};

export function CommunityApplyErrorScreen({
  error,
  onChangeCommunity,
  onRetry,
}: CommunityApplyErrorScreenProps) {
  const systemColorScheme = useSystemColorScheme();
  const [isSigningOut, setIsSigningOut] = React.useState(false);
  const [signOutError, setSignOutError] = React.useState<string | null>(null);
  const signedIn = useHasSignedInSession();

  // The server may be unreachable here: sign out on this computer only
  // (nothing waits on the network, nothing is deleted); never the full reset.
  const signOut = React.useCallback(async () => {
    setSignOutError(null);
    setIsSigningOut(true);
    try {
      await signOutActiveAccountOnThisDevice();
    } catch (cause) {
      setSignOutError(
        cause instanceof Error
          ? cause.message
          : "Couldn't sign out. Try again.",
      );
      setIsSigningOut(false);
    }
  }, []);

  return (
    <div
      className="kuzz-onboarding-neutral-theme kuzz-startup-shell flex items-center justify-center bg-background px-4 py-8 text-foreground"
      data-system-color-scheme={systemColorScheme}
      data-testid="community-apply-error"
    >
      <StartupWindowDragRegion />
      <div className="relative flex w-full max-w-[500px] flex-col items-center text-center">
        <h1 className="text-3xl font-semibold tracking-tight">
          Community connection failed
        </h1>
        <p className="mt-3 text-sm leading-6 text-muted-foreground">{error}</p>
        {onChangeCommunity ? null : (
          <p className="mt-2 text-sm leading-6 text-muted-foreground">
            Check your internet connection and try again.
          </p>
        )}
        <div className="mt-8 flex w-full max-w-[300px] flex-col gap-3">
          <Button
            className="h-10 w-full"
            data-testid="community-apply-error-retry"
            onClick={onRetry}
            type="button"
          >
            Retry
          </Button>
          {onChangeCommunity ? (
            <Button
              className="h-10 w-full"
              onClick={onChangeCommunity}
              type="button"
              variant="secondary"
            >
              Change community
            </Button>
          ) : signedIn ? (
            <Button
              className="h-10 w-full"
              data-testid="community-apply-error-sign-out"
              disabled={isSigningOut}
              onClick={() => void signOut()}
              type="button"
              variant="secondary"
            >
              Sign out
            </Button>
          ) : null}
          {signOutError ? (
            <p className="text-sm text-destructive" role="alert">
              {signOutError}
            </p>
          ) : null}
        </div>
      </div>
    </div>
  );
}
