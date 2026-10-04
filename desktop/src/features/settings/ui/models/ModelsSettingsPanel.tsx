import * as React from "react";

import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/shared/ui/tabs";

import {
  clearRequestedModelsSettingsTab,
  type ModelsSettingsTab,
  readRequestedModelsSettingsTab,
} from "../../lib/modelsSettingsTabRequest";
import { SettingsSectionHeader } from "../SettingsSectionHeader";
import { ProvidersSettingsTab } from "./ProvidersSettingsTab";
import { TaskModelsSettingsTab } from "./TaskModelsSettingsTab";

/**
 * Settings › Models: the AI Buzz itself relies on.
 *
 * - **Providers** — per provider, the subscription sign-in (the harness whose
 *   CLI login bills it) and the API key, plus the single "Default for new
 *   agents" choice. A provider-centric view over `GlobalAgentConfig`.
 * - **Task models** — which provider/model each app task uses (today:
 *   Smart routing's message router).
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
        description="AI that Buzz itself uses — which provider you pay for, and which model handles each app task."
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
            Providers
          </TabsTrigger>
          <TabsTrigger data-testid="settings-models-tab-tasks" value="tasks">
            Task models
          </TabsTrigger>
        </TabsList>
        <TabsContent className="mt-0" value="providers">
          <ProvidersSettingsTab />
        </TabsContent>
        <TabsContent className="mt-0" value="tasks">
          <TaskModelsSettingsTab onOpenProviders={() => setTab("providers")} />
        </TabsContent>
      </Tabs>
    </section>
  );
}
