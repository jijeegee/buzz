import { AgentDefaultsSettingsCard } from "./AgentDefaultsSettingsCard";
import { defaultAiStatusCopy } from "@/features/agents/lib/defaultAi";
import {
  setDefaultAiAutoJoin,
  useDefaultAiAutoJoin,
} from "@/features/agents/lib/defaultAiPreferences";
import { useDefaultAi } from "@/features/agents/useDefaultAi";
import {
  setKeepMentionedAgentsPinned,
  useKeepMentionedAgentsPinned,
} from "@/features/messages/lib/autoPinMentionedAgentsPreference";
import { Switch } from "@/shared/ui/switch";
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
  const addDefaultAiToNewChannels = useDefaultAiAutoJoin();
  const { defaultAi, isLoading: isDefaultAiLoading } = useDefaultAi();

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
          <SettingsOptionRow data-testid="settings-default-ai-auto-join">
            <div className="min-w-0">
              <label
                className="font-medium text-foreground"
                htmlFor="settings-default-ai-auto-join-switch"
              >
                Add default AI to new channels
              </label>
              <p
                className="mt-0.5 text-sm text-muted-foreground/70"
                data-settings-subcopy
              >
                Channels, forums, and project channels you create start with
                your default AI as a bot. Each create form can still opt out.
              </p>
              {isDefaultAiLoading ? null : (
                <p
                  className="mt-0.5 text-sm text-muted-foreground/70"
                  data-settings-subcopy
                  data-testid="settings-default-ai-current"
                >
                  {defaultAiStatusCopy(defaultAi)}
                </p>
              )}
            </div>
            <Switch
              checked={addDefaultAiToNewChannels}
              data-testid="settings-default-ai-auto-join-switch"
              id="settings-default-ai-auto-join-switch"
              onCheckedChange={setDefaultAiAutoJoin}
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
