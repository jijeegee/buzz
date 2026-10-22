import { useT } from "@/shared/i18n";
import { RecoveryScreen } from "./RecoveryScreen";

export function ResetFailedScreen() {
  const t = useT();
  return (
    <RecoveryScreen
      testId="reset-failed"
      title={t("onboarding.resetFailed.title")}
      body={t("onboarding.resetFailed.body")}
    />
  );
}
