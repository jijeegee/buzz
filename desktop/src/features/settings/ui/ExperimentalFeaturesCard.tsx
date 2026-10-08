import { useAppNavigation } from "@/app/navigation/useAppNavigation";
import {
  setContextGaugeEnabled,
  useContextGaugeEnabled,
} from "@/features/agents/lib/contextGaugePreference";
import { ChannelRoutingCard } from "@/features/agents/ui/routing/ChannelRoutingCard";
import { useManagedAgentActions } from "@/features/agents/ui/useManagedAgentActions";
import { setAgentManagedProfiles } from "@/shared/api/tauriWorkspace";
import {
  desktopFeatures,
  useFeatureEnabled,
  useFeatureToggle,
} from "@/shared/features";
import type { FeatureDefinition } from "@/shared/features";
import { Switch } from "@/shared/ui/switch";
import {
  SettingsOptionGroup,
  SettingsOptionGroupList,
  SettingsOptionRow,
} from "./SettingsOptionGroup";
import { SettingsSectionHeader } from "./SettingsSectionHeader";
import { TaskThreadsExperimentGroup } from "./TaskThreadsExperimentGroup";

function FeatureRow({ feature }: { feature: FeatureDefinition }) {
  const [enabled, toggle] = useFeatureToggle(feature.id);
  const switchId = `feature-toggle-${feature.id}`;

  return (
    <SettingsOptionRow>
      <div className="min-w-0 flex-1">
        <p className="text-sm font-medium" id={`${switchId}-label`}>
          {feature.name}
        </p>
        <p className="text-xs text-muted-foreground/70" data-settings-subcopy>
          {feature.description}
        </p>
      </div>
      <Switch
        aria-labelledby={`${switchId}-label`}
        checked={enabled}
        data-testid={switchId}
        onCheckedChange={(value) => {
          toggle(value);
          if (feature.id === "agentManagedProfiles") {
            void setAgentManagedProfiles(value).catch((error) => {
              console.error(
                "Failed to apply agent-managed profiles setting:",
                error,
              );
            });
          }
        }}
      />
    </SettingsOptionRow>
  );
}

function ContextGaugeRow() {
  const contextGaugeEnabled = useContextGaugeEnabled();

  return (
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
  );
}

function ExperimentsChannelRouting() {
  const { goSettings } = useAppNavigation();
  const agents = useManagedAgentActions();

  return (
    <div className="space-y-2">
      <ChannelRoutingCard
        agents={agents.managedAgents}
        onOpenModelsSettings={() => void goSettings("models")}
        onRestartAgent={(pubkey) => {
          void agents.handleRestart(pubkey);
        }}
        restartingAgentPubkey={agents.restartingAgentPubkey}
      />
      {agents.actionErrorMessage ? (
        <p className="px-4 text-sm text-destructive" role="alert">
          {agents.actionErrorMessage}
        </p>
      ) : null}
    </div>
  );
}

export function ExperimentalFeaturesCard() {
  // Manifest is preview-only by definition; every desktop entry is a preview
  // feature.
  const previewFeatures = desktopFeatures;
  // Same gate as Settings › Agents and Models.
  const managedAgentsEnabled = useFeatureEnabled("managed-agents");

  return (
    <section className="min-w-0" data-testid="settings-experimental">
      <SettingsSectionHeader
        title="Experiments"
        description={
          <>
            These features are functional but still being refined. Enable them
            to try new capabilities early.
          </>
        }
      />

      <SettingsOptionGroupList>
        <SettingsOptionGroup title="Features">
          {previewFeatures.map((f) => (
            <FeatureRow feature={f} key={f.id} />
          ))}
        </SettingsOptionGroup>
        {managedAgentsEnabled ? (
          <>
            <SettingsOptionGroup title="Agents">
              <ContextGaugeRow />
            </SettingsOptionGroup>
            <TaskThreadsExperimentGroup />
            <ExperimentsChannelRouting />
          </>
        ) : null}
      </SettingsOptionGroupList>
    </section>
  );
}
