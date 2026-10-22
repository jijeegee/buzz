import * as React from "react";

import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/shared/ui/tabs";

import {
  clearRequestedModelsSettingsTab,
  type ModelsSettingsTab,
  readRequestedModelsSettingsTab,
} from "../../lib/modelsSettingsTabRequest";
import { AgentDefaultsSettingsCard } from "../AgentDefaultsSettingsCard";
import { SettingsSectionHeader } from "../SettingsSectionHeader";
import { ProvidersSettingsTab } from "./ProvidersSettingsTab";
import { TaskModelsSettingsTab } from "./TaskModelsSettingsTab";

/**
 * Settings › Models: which AI Kuzz and your agents run on.
 *
 * - **Connections** — per provider, the subscription sign-in (the harness
 *   whose CLI login bills it) and the API key.
 * - **Default for new agents** — the runtime, provider, model and effort a
 *   new agent starts with.
 * - **App tasks** — which provider/model each app task uses (today:
 *   Smart routing's message router).
 *
 * Connections and the defaults both write `GlobalAgentConfig`; only the
 * active tab is mounted, so each reads the saved record fresh when opened.
 */
export function ModelsSettingsPanel() {
  const [tab, setTab] = React.useState<ModelsSettingsTab>(
    () => readRequestedModelsSettingsTab() ?? "providers",
  );
  React.useEffect(() => clearRequestedModelsSettingsTab(), []);

  return (
    <section className="min-w-0" data-testid="settings-models">
      <SettingsSectionHeader
        title="Models"
        description="Connect the AI you pay for, pick what new agents start with, and choose the model for each app task."
      />
      <Tabs
        onValueChange={(value) => setTab(value as ModelsSettingsTab)}
        value={tab}
      >
        <TabsList aria-label="Models settings" className="mb-6">
          <TabsTrigger
            data-testid="settings-models-tab-providers"
            value="providers"
          >
            Connections
          </TabsTrigger>
          <TabsTrigger
            data-testid="settings-models-tab-defaults"
            value="defaults"
          >
            Default for new agents
          </TabsTrigger>
          <TabsTrigger data-testid="settings-models-tab-tasks" value="tasks">
            App tasks
          </TabsTrigger>
        </TabsList>
        <TabsContent className="mt-0" value="providers">
          <ProvidersSettingsTab />
        </TabsContent>
        <TabsContent className="mt-0" value="defaults">
          <AgentDefaultsSettingsCard />
        </TabsContent>
        <TabsContent className="mt-0" value="tasks">
          <TaskModelsSettingsTab onOpenProviders={() => setTab("providers")} />
        </TabsContent>
      </Tabs>
    </section>
  );
}
