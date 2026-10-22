import { useT } from "@/shared/i18n";
import { RecoveryScreen } from "./RecoveryScreen";

export function RelaunchRequiredScreen() {
  const t = useT();
  return (
    <RecoveryScreen
      testId="relaunch-required"
      title={t("onboarding.relaunchRequired.title")}
      body={t("onboarding.relaunchRequired.body")}
    />
  );
}
