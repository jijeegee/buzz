import type { useGoogleLogin } from "@/shared/hooks/useGoogleLogin";
import { Button } from "./button";

/** Recovery actions shared by onboarding and account settings. */
export function GoogleLoginProgress({
  login,
}: {
  login: ReturnType<typeof useGoogleLogin>;
}) {
  return (
    <div className="space-y-2">
      {login.phase ? (
        <p className="text-sm text-muted-foreground" role="status">
          {login.phase === "completing"
            ? "Finishing sign-in in Buzz…"
            : login.phase === "opening"
              ? "Opening your browser…"
              : "Finish signing in in your browser. If you closed it, cancel and try again."}
        </p>
      ) : null}
      {login.phase === "waiting" ? (
        <Button
          type="button"
          variant="outline"
          onClick={() => void login.cancel()}
        >
          Cancel sign-in
        </Button>
      ) : null}
      {login.error ? (
        <p className="text-sm text-destructive" role="alert">
          {login.error}
        </p>
      ) : null}
    </div>
  );
}
