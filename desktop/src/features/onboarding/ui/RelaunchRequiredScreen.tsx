import { RecoveryScreen } from "./RecoveryScreen";

export function RelaunchRequiredScreen() {
  return (
    <RecoveryScreen
      testId="relaunch-required"
      title="Restart Kuzz to finish recovery"
      body="Your identity was updated. Kuzz needs to restart so syncing and agents run under it."
    />
  );
}
