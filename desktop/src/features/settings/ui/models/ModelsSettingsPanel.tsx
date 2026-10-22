import * as React from "react";

import { useT } from "@/shared/i18n";
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
  const t = useT();
  const [tab, setTab] = React.useState<ModelsSettingsTab>(
    () => readRequestedModelsSettingsTab() ?? "providers",
  );
  React.useEffect(() => clearRequestedModelsSettingsTab(), []);

  return (
    <section className="min-w-0" data-testid="settings-models">
      <SettingsSectionHeader
        title={t("settings.models.title")}
        description={t("settings.models.description")}
      />
      <Tabs
        onValueChange={(value) => setTab(value as ModelsSettingsTab)}
        value={tab}
      >
        <TabsList aria-label={t("settings.models.tabsLabel")} className="mb-6">
          <TabsTrigger
            data-testid="settings-models-tab-providers"
            value="providers"
          >
            {t("settings.models.tabConnections")}
          </TabsTrigger>
          <TabsTrigger
            data-testid="settings-models-tab-defaults"
            value="defaults"
          >
            {t("settings.models.tabDefaults")}
          </TabsTrigger>
          <TabsTrigger data-testid="settings-models-tab-tasks" value="tasks">
            {t("settings.models.tabTasks")}
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
