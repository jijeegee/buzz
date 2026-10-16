import * as React from "react";
import { useBuiltInServer } from "@/features/communities/builtInServer";
import { getTokenAuthStatus } from "@/shared/api/tokenAuth";
import { Button } from "@/shared/ui/button";
import { useCommunityOnboarding } from "../communityOnboarding";
import { OnboardingCard } from "./OnboardingCard";

/** Connect the first community at the native-configured custody service origin. */
export function DefaultCommunitySetup({
  onManualSetup,
  onBack,
}: {
  onManualSetup: () => void;
  onBack: () => void;
}) {
  const { start } = useCommunityOnboarding();
  const [attempt, retry] = React.useReducer((value: number) => value + 1, 0);
  const [error, setError] = React.useState<string | null>(null);
  // One built-in server: no manual setup to fall back to.
  const singleServer = useBuiltInServer() !== null;
  const fallbackHint = singleServer
    ? "Check your internet connection and try again."
    : "Try again or use advanced setup.";

  // biome-ignore lint/correctness/useExhaustiveDependencies: an explicit retry starts a new bounded attempt.
  React.useEffect(() => {
    let cancelled = false;
    setError(null);
    const timer = window.setTimeout(() => {
      cancelled = true;
      setError(`The default community did not respond. ${fallbackHint}`);
    }, 15_000);
    void getTokenAuthStatus()
      .then(({ origin }) => {
        if (cancelled) return;
        const url = new URL(origin);
        if (
          !["https:", "http:"].includes(url.protocol) ||
          url.username ||
          url.password ||
          url.search ||
          url.hash ||
          url.pathname !== "/"
        ) {
          throw new Error("Invalid community service origin");
        }
        url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
        start({ source: "default-community", relayUrl: url.toString() });
      })
      .catch(() => {
        if (!cancelled) {
          setError(`Could not prepare the default community. ${fallbackHint}`);
        }
      })
      .finally(() => window.clearTimeout(timer));
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [attempt, fallbackHint, start]);

  return (
    <OnboardingCard
      current={5}
      backAction={{ onClick: onBack }}
      testId="default-community-setup"
    >
      <h1 className="text-title font-normal">Connecting to your community</h1>
      <p className="mt-3 text-sm" role={error ? "alert" : "status"}>
        {error ?? "Preparing your default community…"}
      </p>
      <div className="mt-6 flex justify-center gap-3">
        {error ? <Button onClick={retry}>Retry</Button> : null}
        {singleServer ? null : (
          <Button variant="ghost" onClick={onManualSetup}>
            Advanced setup
          </Button>
        )}
      </div>
    </OnboardingCard>
  );
}
