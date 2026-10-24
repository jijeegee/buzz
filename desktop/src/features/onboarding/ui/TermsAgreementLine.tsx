import type { ReactNode } from "react";

import { openUrl } from "@tauri-apps/plugin-opener";

import { useT } from "@/shared/i18n";
import {
  PRIVACY_POLICY_URL,
  TERMS_OF_SERVICE_URL,
} from "@/shared/lib/kuzzLinks";
import { Button } from "@/shared/ui/button";

function PolicyLink({ url, children }: { url: string; children: ReactNode }) {
  return (
    <Button
      className="h-auto p-0 align-baseline text-xs no-underline hover:underline focus-visible:no-underline"
      onClick={() => void openUrl(url).catch(() => {})}
      type="button"
      variant="link"
    >
      {children}
    </Button>
  );
}

/** The one line under "Sign in with Google": continuing means agreeing. */
export function TermsAgreementLine() {
  const t = useT();
  return (
    <p
      className="text-xs leading-5 text-muted-foreground"
      data-testid="sign-in-terms"
    >
      {t("onboarding.termsLine.text", {
        terms: (chunks) => (
          <PolicyLink key="terms" url={TERMS_OF_SERVICE_URL}>
            {chunks}
          </PolicyLink>
        ),
        privacy: (chunks) => (
          <PolicyLink key="privacy" url={PRIVACY_POLICY_URL}>
            {chunks}
          </PolicyLink>
        ),
      })}
    </p>
  );
}
