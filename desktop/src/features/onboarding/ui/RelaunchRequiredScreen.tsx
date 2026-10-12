import { RecoveryScreen } from "./RecoveryScreen";

export function RelaunchRequiredScreen() {
  return (
    <RecoveryScreen
      testId="relaunch-required"
      title="Restart kuzz to finish recovery"
      body="Your identity was updated. kuzz needs to restart so syncing and agents run under it."
    />
  );
}
