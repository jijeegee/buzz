import { openUrl } from "@tauri-apps/plugin-opener";

import { useT } from "@/shared/i18n";
import { TERMS_OF_SERVICE_URL } from "@/shared/lib/kuzzLinks";
import { Button } from "@/shared/ui/button";

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
          <Button
            className="h-auto p-0 align-baseline text-xs no-underline hover:underline focus-visible:no-underline"
            onClick={() => void openUrl(TERMS_OF_SERVICE_URL).catch(() => {})}
            type="button"
            variant="link"
          >
            {chunks}
          </Button>
        ),
      })}
    </p>
  );
}
