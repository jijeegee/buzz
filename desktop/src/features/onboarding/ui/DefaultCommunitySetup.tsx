import * as React from "react";
import { useBuiltInServer } from "@/features/communities/builtInServer";
import { getTokenAuthStatus } from "@/shared/api/tokenAuth";
import { useT } from "@/shared/i18n";
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
  const t = useT();
  const { start } = useCommunityOnboarding();
  const [attempt, retry] = React.useReducer((value: number) => value + 1, 0);
  const [error, setError] = React.useState<string | null>(null);
  // One built-in server: no manual setup to fall back to.
  const singleServer = useBuiltInServer() !== null;
  const noResponseMessage = singleServer
    ? t("onboarding.defaultCommunity.noResponseOffline")
    : t("onboarding.defaultCommunity.noResponseAdvanced");
  const prepareFailedMessage = singleServer
    ? t("onboarding.defaultCommunity.prepareFailedOffline")
    : t("onboarding.defaultCommunity.prepareFailedAdvanced");

  // biome-ignore lint/correctness/useExhaustiveDependencies: an explicit retry starts a new bounded attempt.
  React.useEffect(() => {
    let cancelled = false;
    setError(null);
    const timer = window.setTimeout(() => {
      cancelled = true;
      setError(noResponseMessage);
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
          setError(prepareFailedMessage);
        }
      })
      .finally(() => window.clearTimeout(timer));
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [attempt, noResponseMessage, prepareFailedMessage, start]);

  return (
    <OnboardingCard
      current={5}
      backAction={{ onClick: onBack }}
      testId="default-community-setup"
    >
      <h1 className="text-title font-normal">
        {t("onboarding.defaultCommunity.title")}
      </h1>
      <p className="mt-3 text-sm" role={error ? "alert" : "status"}>
        {error ?? t("onboarding.defaultCommunity.preparing")}
      </p>
      <div className="mt-6 flex justify-center gap-3">
        {error ? <Button onClick={retry}>{t("common.retry")}</Button> : null}
        {singleServer ? null : (
          <Button variant="ghost" onClick={onManualSetup}>
            {t("onboarding.defaultCommunity.advancedSetup")}
          </Button>
        )}
      </div>
    </OnboardingCard>
  );
}
