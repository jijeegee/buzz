import { AgentDefaultsSettingsCard } from "./AgentDefaultsSettingsCard";
import {
  setContextGaugeEnabled,
  useContextGaugeEnabled,
} from "@/features/agents/lib/contextGaugePreference";
import {
  setKeepMentionedAgentsPinned,
  useKeepMentionedAgentsPinned,
} from "@/features/messages/lib/autoPinMentionedAgentsPreference";
import { Switch } from "@/shared/ui/switch";
import { ChannelRoutingSummaryRow } from "./ChannelRoutingSummaryRow";
import { HarnessesSettingsPanel } from "./HarnessesSettingsPanel";
import { PreventSleepSettingsCard } from "./PreventSleepSettingsCard";
import {
  SettingsOptionGroup,
  SettingsOptionGroupList,
  SettingsOptionRow,
} from "./SettingsOptionGroup";
import { SettingsSectionHeader } from "./SettingsSectionHeader";

export function AgentsSettingsPanel() {
  const automaticallyMentionAgents = useKeepMentionedAgentsPinned();
  const contextGaugeEnabled = useContextGaugeEnabled();

  return (
    <section className="min-w-0" data-testid="settings-agents">
      <SettingsSectionHeader
        title="Agents"
        description="Control how agents behave in conversations and run on this machine."
      />

      <SettingsOptionGroupList>
        <SettingsOptionGroup title="Conversations">
          <SettingsOptionRow data-testid="settings-automatic-agent-mentions">
            <div className="min-w-0">
              <label
                className="font-medium text-foreground"
                htmlFor="settings-automatic-agent-mentions-switch"
              >
                Automatically mention agents
              </label>
              <p
                className="mt-0.5 text-sm text-muted-foreground/70"
                data-settings-subcopy
              >
                Address selected agents in thread replies
              </p>
            </div>
            <Switch
              aria-label="Automatically mention agents"
              checked={automaticallyMentionAgents}
              id="settings-automatic-agent-mentions-switch"
              onCheckedChange={setKeepMentionedAgentsPinned}
            />
          </SettingsOptionRow>
          <ChannelRoutingSummaryRow />
          <SettingsOptionRow data-testid="settings-context-gauge">
            <div className="min-w-0">
              <label
                className="font-medium text-foreground"
                htmlFor="settings-context-gauge-switch"
              >
                Show context gauge
              </label>
              <p
                className="mt-0.5 text-sm text-muted-foreground/70"
                data-settings-subcopy
              >
                Show each session's context usage on agent avatars, with Compact
              </p>
            </div>
            <Switch
              aria-label="Show context gauge"
              checked={contextGaugeEnabled}
              id="settings-context-gauge-switch"
              onCheckedChange={setContextGaugeEnabled}
            />
          </SettingsOptionRow>
        </SettingsOptionGroup>
        <PreventSleepSettingsCard />
        <HarnessesSettingsPanel />
        <AgentDefaultsSettingsCard />
      </SettingsOptionGroupList>
    </section>
  );
}
