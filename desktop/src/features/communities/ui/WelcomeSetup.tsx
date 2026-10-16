import * as React from "react";
import { Check, Copy } from "lucide-react";

import {
  type FirstCommunityPage,
  useCommunityOnboarding,
} from "@/features/onboarding/communityOnboarding";
import { InviteRedeemForm } from "@/features/onboarding/ui/InviteRedeemForm";
import { OnboardingChrome } from "@/features/onboarding/ui/OnboardingChrome";
import { OnboardingFooterProvider } from "@/features/onboarding/ui/OnboardingFooter";
import {
  type OnboardingTransitionDirection,
  OnboardingSlideTransition,
} from "@/features/onboarding/ui/OnboardingSlideTransition";
import { useIdentityQuery } from "@/shared/api/hooks";
import { writeTextToClipboard } from "@/shared/lib/clipboard";
import { pubkeyToNpub } from "@/shared/lib/nostrUtils";
import { signingKeyUiTestSeam } from "@/shared/lib/signingKeyUi";
import { useSystemColorScheme } from "@/shared/theme/useSystemColorScheme";
import { Button } from "@/shared/ui/button";
import { Card } from "@/shared/ui/card";
import { StartupWindowDragRegion } from "@/shared/ui/StartupWindowDragRegion";

type WelcomeSetupPage = "welcome" | "join" | "member";
type WelcomeTransitionMode = "initial" | OnboardingTransitionDirection;

type WelcomeSetupProps = {
  initialPage?: WelcomeSetupPage | FirstCommunityPage;
  initialTransitionMode?: WelcomeTransitionMode;
  onBack?: () => void;
};

const COMMUNITY_OPTION_CARD_CLASS =
  "w-full max-w-[320px] items-center px-6 py-4 text-center text-sm font-normal leading-6 text-foreground [--buzz-card-textured-min-height:88px] transition-[filter] duration-150 ease-out hover:brightness-[0.98] focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-foreground/35";

export function WelcomeSetup({
  initialPage = "welcome",
  initialTransitionMode = "initial",
  onBack,
}: WelcomeSetupProps) {
  // "owned" (Builderlab-hosted) is not offered in Kuzz; a resumed owned
  // transaction falls back to the choices page.
  const [page, setPage] = React.useState<WelcomeSetupPage>(
    initialPage === "owned" ? "welcome" : initialPage,
  );
  const [transitionMode, setTransitionMode] =
    React.useState<WelcomeTransitionMode>(initialTransitionMode);
  const [copiedNpub, setCopiedNpub] = React.useState(false);
  const communityOnboarding = useCommunityOnboarding();
  const identityQuery = useIdentityQuery();
  const systemColorScheme = useSystemColorScheme();
  const npub = identityQuery.data?.pubkey
    ? pubkeyToNpub(identityQuery.data.pubkey)
    : "";
  const npubError = identityQuery.error
    ? identityQuery.error instanceof Error
      ? identityQuery.error.message
      : "Couldn't load your account."
    : null;

  const showPage = React.useCallback(
    (nextPage: WelcomeSetupPage, direction?: OnboardingTransitionDirection) => {
      setTransitionMode(
        direction ?? (nextPage === "welcome" ? "backward" : "forward"),
      );
      setPage(nextPage);
    },
    [],
  );

  const startConnection = React.useCallback(
    (relayUrl: string) => {
      communityOnboarding.start({
        source: "first-community",
        firstCommunityPage: page === "member" ? "member" : "join",
        relayUrl,
      });
    },
    [communityOnboarding, page],
  );

  const redeemInvite = React.useCallback(
    (relayUrl: string, code: string, policyReceipt?: string) => {
      communityOnboarding.start({
        source: "first-community",
        firstCommunityPage: page === "member" ? "member" : "join",
        relayUrl,
        inviteCode: code,
        policyReceipt,
      });
    },
    [communityOnboarding, page],
  );

  const transitionDirection =
    transitionMode === "backward" ? "backward" : "forward";
  const backAction =
    page === "welcome" && onBack
      ? { onClick: onBack, testId: "welcome-setup-back" }
      : page === "join"
        ? {
            onClick: () => showPage("welcome"),
            testId: "welcome-join-back",
          }
        : page === "member"
          ? {
              onClick: () => showPage("welcome"),
              testId: "welcome-member-back",
            }
          : undefined;

  return (
    <div
      className="buzz-onboarding-neutral-theme buzz-startup-shell flex h-dvh items-start justify-center overflow-y-auto bg-background px-4 pb-36 pt-[106px] text-foreground"
      data-system-color-scheme={systemColorScheme}
      data-testid="welcome-setup"
    >
      <StartupWindowDragRegion />
      <OnboardingChrome current={5} />
      <OnboardingFooterProvider backAction={backAction}>
        <div className="relative flex min-h-0 w-full max-w-[920px] flex-1 flex-col items-center text-center">
          {page === "welcome" ? (
            <OnboardingSlideTransition
              className="flex h-full min-h-0 w-full flex-col items-center text-center"
              containerClassName="h-full min-h-0 [&>.buzz-onboarding-transition-line]:h-full"
              direction={transitionDirection}
              transitionKey={`welcome-${transitionDirection}`}
            >
              <div className="w-full max-w-[760px]">
                <h1 className="text-title font-normal">Find your community</h1>
                <p className="mt-3 text-sm leading-6 text-foreground/80">
                  Join with an invite, or reconnect to a community you already
                  have.
                </p>
              </div>
              <div className="flex w-full flex-1 translate-y-16 flex-col items-center justify-center gap-20 py-8">
                <Card
                  asChild
                  className={COMMUNITY_OPTION_CARD_CLASS}
                  variant="textured"
                >
                  <button
                    data-testid="community-choice-join"
                    onClick={() => showPage("join")}
                    type="button"
                  >
                    Join a community
                  </button>
                </Card>
                <Card
                  asChild
                  className={COMMUNITY_OPTION_CARD_CLASS}
                  variant="textured"
                >
                  <button
                    data-testid="community-choice-existing"
                    onClick={() => showPage("member")}
                    type="button"
                  >
                    I already have a community
                  </button>
                </Card>
              </div>
            </OnboardingSlideTransition>
          ) : (
            <OnboardingSlideTransition
              className="flex min-h-[calc(100dvh-15.625rem)] w-full flex-col items-center text-center"
              direction={transitionDirection}
              transitionKey={`${page}-${transitionDirection}`}
            >
              <div className="w-full max-w-[620px]">
                <h1 className="text-title font-normal">
                  {page === "member"
                    ? "Reconnect to your community"
                    : "Join a community"}
                </h1>
                <p className="mt-3 text-sm leading-6 text-foreground/80">
                  {page === "member"
                    ? "Enter the community URL or an invite link. Your role will be restored when you connect."
                    : "Enter the invite link or community URL you received."}
                </p>
              </div>
              <div className="flex w-full flex-1 flex-col items-center justify-center gap-16">
                <InviteRedeemForm
                  error={null}
                  isRedeeming={false}
                  onCancel={() => showPage("welcome")}
                  onConnect={startConnection}
                  onRedeem={redeemInvite}
                  placeholder="Invite link or community URL"
                  variant="onboarding-spotlight"
                />
                {/* Kuzz never shows the account's public ID; E2E builds keep
                    the copy block. */}
                {page === "join" && signingKeyUiTestSeam() ? (
                  <div className="w-full max-w-[560px] text-left">
                    <p className="text-sm font-medium text-foreground">
                      Joining a private community?
                    </p>
                    <p className="mt-2 text-sm leading-6 text-foreground/75">
                      Some communities need the owner to add you before you can
                      join. Copy your public ID and send it to the community
                      owner.
                    </p>
                    <div className="mt-4 flex items-center gap-3 rounded-xl border border-foreground/10 bg-background/35 px-4 py-3">
                      <code
                        className="min-w-0 flex-1 truncate font-mono text-xs text-foreground/80"
                        data-testid="welcome-join-npub"
                      >
                        {npub || "Loading…"}
                      </code>
                      <Button
                        aria-label="Copy public ID"
                        className="h-9 shrink-0 rounded-full px-3"
                        disabled={!npub}
                        onClick={() => {
                          void writeTextToClipboard(npub).then(() => {
                            setCopiedNpub(true);
                            window.setTimeout(() => setCopiedNpub(false), 1500);
                          });
                        }}
                        size="sm"
                        type="button"
                        variant="outline"
                      >
                        {copiedNpub ? (
                          <Check className="h-4 w-4" aria-hidden="true" />
                        ) : (
                          <Copy className="h-4 w-4" aria-hidden="true" />
                        )}
                        <span>{copiedNpub ? "Copied" : "Copy"}</span>
                      </Button>
                    </div>
                    {npubError ? (
                      <p className="mt-3 text-sm text-destructive">
                        {npubError}
                      </p>
                    ) : null}
                  </div>
                ) : null}
              </div>
            </OnboardingSlideTransition>
          )}
        </div>
      </OnboardingFooterProvider>
    </div>
  );
}
