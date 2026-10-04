import { LogIn, LogOut } from "lucide-react";
import * as React from "react";
import { toast } from "sonner";

import {
  startGoogleSignIn,
  useWebSession,
  webSession,
} from "@/shared/auth/session";
import { Button } from "@/shared/ui/button";

/**
 * Sign-in / sign-out control, rendered only on relays that advertise token
 * auth. Key-mode relays show nothing (today's behavior).
 */
export function AccountControl() {
  const session = useWebSession();
  const [busy, setBusy] = React.useState(false);

  if (
    !session.tokenAuth?.oidcProviders.includes("google") ||
    window.location.pathname.startsWith("/auth/")
  ) {
    return null;
  }

  const signIn = async () => {
    setBusy(true);
    try {
      await startGoogleSignIn();
    } catch (error) {
      setBusy(false);
      toast.error(error instanceof Error ? error.message : "Sign-in failed.");
    }
  };

  const signOut = async () => {
    setBusy(true);
    try {
      await webSession.signOut();
    } catch (error) {
      toast.error(error instanceof Error ? error.message : "Sign-out failed.");
    } finally {
      setBusy(false);
    }
  };

  if (session.status === "unknown") return null;

  if (session.status === "signed_in") {
    return (
      <div className="flex items-center gap-3 text-sm">
        <span className="text-muted-foreground">
          {session.displayName
            ? `Signed in as ${session.displayName}`
            : "Signed in"}
        </span>
        <Button
          disabled={busy}
          onClick={signOut}
          size="sm"
          type="button"
          variant="outline"
        >
          <LogOut aria-hidden="true" />
          Sign out
        </Button>
      </div>
    );
  }

  return (
    <div className="flex items-center gap-3 text-sm">
      {session.status === "unavailable" ? (
        <span className="text-muted-foreground" role="status">
          Can&apos;t reach the server to restore your session.
        </span>
      ) : null}
      <Button
        disabled={busy}
        onClick={signIn}
        size="sm"
        type="button"
        variant="outline"
      >
        <LogIn aria-hidden="true" />
        Sign in with Google
      </Button>
    </div>
  );
}
