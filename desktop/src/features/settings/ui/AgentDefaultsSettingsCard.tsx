import { AgentDefaultsEditor } from "@/features/agents/ui/AgentDefaultsEditor";
import { SettingsOptionGroup } from "./SettingsOptionGroup";
import { useT } from "@/shared/i18n";

export function AgentDefaultsSettingsCard() {
  const t = useT();
  return (
    <SettingsOptionGroup
      data-testid="settings-global-agent-config"
      description={t("settings.agentDefaults.description")}
      title={t("settings.agentDefaults.title")}
    >
      <div className="px-4 py-4">
        <AgentDefaultsEditor layout="flat" />
      </div>
    </SettingsOptionGroup>
  );
}
