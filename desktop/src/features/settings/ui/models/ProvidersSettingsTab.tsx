import * as React from "react";
import { AlertCircle } from "lucide-react";

import {
  useAcpRuntimesQueryForced,
  useBakedBuildEnvKeysQuery,
  useSetGlobalAgentConfigMutation,
} from "@/features/agents/hooks";
import { ChatGptPlanSettingsRow } from "@/features/agents/ui/ChatGptPlanControls";
import { BLOCK_BUILD_HIDDEN_PROVIDER_IDS } from "@/features/agents/ui/agentConfigOptions";
import { useGlobalAgentConfig } from "@/features/agents/useGlobalAgentConfig";
import type { GlobalAgentConfig } from "@/shared/api/types";
import { t as translate, useT } from "@/shared/i18n";

import { HarnessesSettingsPanel } from "../HarnessesSettingsPanel";
import {
  SettingsOptionGroup,
  SettingsOptionGroupList,
  SettingsOptionRow,
} from "../SettingsOptionGroup";
import { ProviderApiKeyRow } from "./ProviderApiKeyRow";
import {
  buildProviderRows,
  groupProviderRows,
  nextGlobalConfigForApiKey,
  saveNoticeText,
  type ProviderGroup,
} from "./providersSettingsModel";
import type { SaveNotice } from "./SaveNoticeLine";

type ScopedNotice = SaveNotice & { scope: string };

const NO_HIDDEN_PROVIDERS: ReadonlySet<string> = new Set();

/**
 * Settings › AI connections › Tools & accounts.
 *
 * Two parts:
 * - **Harnesses on this computer** — the unmodified `HarnessesSettingsPanel`
 *   (install, update, connect and sign in to each harness, plus Add harness).
 * - **Accounts Kuzz uses directly** — one block per provider with the
 *   ChatGPT plan row (OpenAI) and the API-key row. Harness sign-in is not
 *   repeated here; it lives in the harness list above.
 *
 * Every account control writes the whole `GlobalAgentConfig` in one
 * `set_global_agent_config`, and the result line says when that restarted
 * running agents.
 */
export function ProvidersSettingsTab() {
  const t = useT();
  const runtimesQuery = useAcpRuntimesQueryForced();
  const bakedEnvKeysQuery = useBakedBuildEnvKeysQuery();
  const {
    globalConfig,
    isError: isConfigError,
    isReady: isConfigReady,
  } = useGlobalAgentConfig();
  const saveMutation = useSetGlobalAgentConfigMutation();
  const [notice, setNotice] = React.useState<ScopedNotice | null>(null);

  const catalog = runtimesQuery.data;
  // Internal Block builds bake a provider and hide Databricks v1 — the same
  // rule the agent dialogs apply.
  const hiddenProviderIds = (bakedEnvKeysQuery.data ?? []).includes(
    "KUZZ_AGENT_PROVIDER",
  )
    ? BLOCK_BUILD_HIDDEN_PROVIDER_IDS
    : NO_HIDDEN_PROVIDERS;
  const rows = React.useMemo(
    () => buildProviderRows(catalog ?? [], globalConfig, hiddenProviderIds),
    [catalog, globalConfig, hiddenProviderIds],
  );
  const groups = React.useMemo(() => groupProviderRows(rows), [rows]);

  // Every write spreads `globalConfig` back into `set_global_agent_config`,
  // so nothing may write until the real record is loaded: the hook's
  // placeholder (and its post-error fallback) is an empty config that would
  // wipe every other key and default.
  const busy = saveMutation.isPending || !isConfigReady;

  const persist = React.useCallback(
    async (scope: string, next: GlobalAgentConfig): Promise<boolean> => {
      if (!isConfigReady) return false;
      setNotice(null);
      try {
        const result = await saveMutation.mutateAsync(next);
        setNotice({ scope, tone: "ok", text: saveNoticeText(result) });
        return true;
      } catch (error) {
        setNotice({
          scope,
          tone: "error",
          text:
            typeof error === "string"
              ? error
              : translate("settings.providers.saveFailed"),
        });
        return false;
      }
    },
    [isConfigReady, saveMutation.mutateAsync],
  );

  const noticeFor = (scope: string) =>
    notice && notice.scope === scope ? notice : null;

  function renderGroup(group: ProviderGroup) {
    const { primary, companions } = group;
    const headingId = `settings-models-provider-${primary.providerId}-heading`;
    return (
      <section
        aria-labelledby={headingId}
        data-testid={`settings-models-provider-${primary.providerId}`}
        key={primary.providerId}
      >
        <h3
          className="px-4 pt-3.5 text-sm font-semibold text-foreground"
          id={headingId}
        >
          {primary.label}
        </h3>
        {primary.providerId === "openai" ? (
          <div>
            <p className="px-4 pt-3 text-xs font-medium text-muted-foreground/70">
              {t("settings.providers.chatgptPlanHeading")}
            </p>
            <ChatGptPlanSettingsRow />
          </div>
        ) : null}
        {primary.apiKeyEnvVar ? (
          <ProviderApiKeyRow
            companions={companions}
            disabled={busy}
            notice={noticeFor(`api-key:${primary.providerId}`)}
            onRemove={() =>
              persist(
                `api-key:${primary.providerId}`,
                nextGlobalConfigForApiKey(
                  globalConfig,
                  primary.apiKeyEnvVar ?? "",
                  "",
                ),
              )
            }
            onSave={(value) =>
              persist(
                `api-key:${primary.providerId}`,
                nextGlobalConfigForApiKey(
                  globalConfig,
                  primary.apiKeyEnvVar ?? "",
                  value,
                ),
              )
            }
            row={primary}
            showEnvVarName={false}
          />
        ) : (
          <SettingsOptionRow
            data-testid={`settings-models-${primary.providerId}-workspace`}
          >
            <div className="min-w-0">
              <p className="font-medium text-foreground">
                {t("settings.providers.workspaceSignIn")}
              </p>
              <p
                className="mt-0.5 text-sm text-muted-foreground/70"
                data-settings-subcopy
              >
                {t("settings.providers.workspaceSignInHint", {
                  provider: primary.label,
                })}
              </p>
            </div>
          </SettingsOptionRow>
        )}
      </section>
    );
  }

  return (
    <div data-testid="settings-models-providers">
      {isConfigError ? (
        <p
          className="mb-6 flex items-center gap-1 text-sm text-destructive"
          data-testid="settings-models-config-error"
          role="alert"
        >
          <AlertCircle aria-hidden="true" className="size-3.5 shrink-0" />
          {t("settings.providers.configLoadFailed")}
        </p>
      ) : null}
      <SettingsOptionGroupList>
        <HarnessesSettingsPanel />
        <SettingsOptionGroup
          data-testid="settings-models-accounts"
          description={t("settings.providers.accountsDescription")}
          title={t("settings.providers.accountsTitle")}
        >
          {groups.map(renderGroup)}
        </SettingsOptionGroup>
      </SettingsOptionGroupList>
    </div>
  );
}
