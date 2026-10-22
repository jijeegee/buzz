import { AgentDefaultsEditor } from "@/features/agents/ui/AgentDefaultsEditor";
import { SettingsOptionGroup } from "./SettingsOptionGroup";

export function AgentDefaultsSettingsCard() {
  return (
    <SettingsOptionGroup
      data-testid="settings-global-agent-config"
      description="What a new agent starts with: runtime, provider, model, effort and environment. An agent's own settings always take priority. Sign-in and API keys live under Connections."
      title="Default for new agents"
    >
      <div className="px-4 py-4">
        <AgentDefaultsEditor layout="flat" />
      </div>
    </SettingsOptionGroup>
  );
}
