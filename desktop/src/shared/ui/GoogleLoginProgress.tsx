import { useT } from "@/shared/i18n";
import type { useGoogleLogin } from "@/shared/hooks/useGoogleLogin";
import { CreateAccountDialog } from "./CreateAccountDialog";

/** Recovery actions shared by onboarding and account settings. */
export function GoogleLoginProgress({
  login,
}: {
  login: ReturnType<typeof useGoogleLogin>;
}) {
  const t = useT();
  return (
    <div className="space-y-2">
      <CreateAccountDialog login={login} />
      {login.phase && login.phase !== "signup" ? (
        <p className="text-sm text-muted-foreground" role="status">
          {login.phase === "completing"
            ? t("ui.googleLogin.finishing")
            : login.phase === "opening"
              ? t("ui.googleLogin.openingBrowser")
              : t("ui.googleLogin.finishInBrowser")}
        </p>
      ) : null}
      {login.error ? (
        <p className="text-sm text-destructive" role="alert">
          {login.error}
        </p>
      ) : null}
    </div>
  );
}
