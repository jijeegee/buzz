import { openUrl } from "@tauri-apps/plugin-opener";

import { TERMS_OF_SERVICE_URL } from "@/shared/lib/kuzzLinks";
import { Button } from "@/shared/ui/button";

/** The one line under "Sign in with Google": continuing means agreeing. */
export function TermsAgreementLine() {
  return (
    <p
      className="text-xs leading-5 text-muted-foreground"
      data-testid="sign-in-terms"
    >
      By continuing you agree to the{" "}
      <Button
        className="h-auto p-0 align-baseline text-xs no-underline hover:underline focus-visible:no-underline"
        onClick={() => void openUrl(TERMS_OF_SERVICE_URL).catch(() => {})}
        type="button"
        variant="link"
      >
        Terms of Service
      </Button>
    </p>
  );
}
