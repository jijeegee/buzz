import * as React from "react";
import { AlertCircle, RefreshCw } from "lucide-react";

import {
  useAcpRuntimesQueryForced,
  useBakedBuildEnvKeysQuery,
  useSetGlobalAgentConfigMutation,
} from "@/features/agents/hooks";
import { BLOCK_BUILD_HIDDEN_PROVIDER_IDS } from "@/features/agents/ui/agentConfigOptions";
import { useGlobalAgentConfig } from "@/features/agents/useGlobalAgentConfig";
import type { GlobalAgentConfig } from "@/shared/api/types";
import { cn } from "@/shared/lib/cn";
import { Button } from "@/shared/ui/button";

import { HarnessRow } from "../HarnessRow";
import {
  SettingsOptionGroup,
  SettingsOptionGroupList,
  SettingsOptionRow,
} from "../SettingsOptionGroup";
import { DefaultForNewAgentsGroup } from "./DefaultForNewAgentsGroup";
import { ProviderApiKeyRow } from "./ProviderApiKeyRow";
import {
  buildProviderRows,
  defaultOptions,
  groupProviderRows,
  nextGlobalConfigForApiKey,
  nextGlobalConfigForDefault,
  saveNoticeText,
  type DefaultOption,
  type ProviderGroup,
} from "./providersSettingsModel";
import type { SaveNotice } from "./SaveNoticeLine";

type ScopedNotice = SaveNotice & { scope: string };

const NO_HIDDEN_PROVIDERS: ReadonlySet<string> = new Set();

/**
 * Settings › Models › Providers.
 *
 * One `SettingsOptionGroup` per provider — its subscription sign-in row (the
 * unmodified `HarnessRow` for the harness whose CLI login bills that
 * provider, found through the catalog's `subscriptionProvider` fact) and its
 * API-key row — followed by the single "Default for new agents" group. Every
 * control writes the whole `GlobalAgentConfig` in one
 * `set_global_agent_config`, and the result line says when that restarted
 * running agents.
 */
export function ProvidersSettingsTab() {
  const runtimesQuery = useAcpRuntimesQueryForced();
  const bakedEnvKeysQuery = useBakedBuildEnvKeysQuery();
  const {
    globalConfig,
    isError: isConfigError,
    isReady: isConfigReady,
  } = useGlobalAgentConfig();
  const saveMutation = useSetGlobalAgentConfigMutation();
  // Bumped by "Check again" so HarnessRow drops stale install results, the
  // same way the Agent runtimes panel does.
  const [resetEpoch, setResetEpoch] = React.useState(0);
  const [notice, setNotice] = React.useState<ScopedNotice | null>(null);

  const catalog = runtimesQuery.data;
  // Internal Block builds bake a provider and hide Databricks v1 — the same
  // rule the agent dialogs apply.
  const hiddenProviderIds = (bakedEnvKeysQuery.data ?? []).includes(
    "BUZZ_AGENT_PROVIDER",
  )
    ? BLOCK_BUILD_HIDDEN_PROVIDER_IDS
    : NO_HIDDEN_PROVIDERS;
  const rows = React.useMemo(
    () => buildProviderRows(catalog ?? [], globalConfig, hiddenProviderIds),
    [catalog, globalConfig, hiddenProviderIds],
  );
  const groups = React.useMemo(() => groupProviderRows(rows), [rows]);
  const options = React.useMemo(() => defaultOptions(rows), [rows]);

  const isRefreshing = runtimesQuery.isFetching;
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
          text: typeof error === "string" ? error : "Couldn't save.",
        });
        return false;
      }
    },
    [isConfigReady, saveMutation.mutateAsync],
  );

  const noticeFor = (scope: string) =>
    notice && notice.scope === scope ? notice : null;

  function handleChooseDefault(option: DefaultOption) {
    const row = rows.find(
      (candidate) => candidate.providerId === option.providerId,
    );
    if (!row) return;
    const next = nextGlobalConfigForDefault(globalConfig, row, option.path);
    if (!next) return;
    void persist("default", next);
  }

  function handleCheckAgain() {
    setResetEpoch((epoch) => epoch + 1);
    void runtimesQuery.forceRefresh();
  }

  function renderGroup(group: ProviderGroup) {
    const { primary, companions } = group;
    const subscription = primary.subscriptionRuntime;
    return (
      <SettingsOptionGroup
        data-testid={`settings-models-provider-${primary.providerId}`}
        headerAction={
          subscription ? (
            <Button
              disabled={isRefreshing}
              onClick={handleCheckAgain}
              size="sm"
              type="button"
              variant="outline"
            >
              <RefreshCw
                className={cn("h-4 w-4", isRefreshing && "animate-spin")}
              />
              Check again
            </Button>
          ) : undefined
        }
        key={primary.providerId}
        title={primary.label}
      >
        {subscription ? (
          <div
            data-testid={`settings-models-${primary.providerId}-subscription`}
          >
            <p className="px-4 pt-3 text-xs font-medium text-muted-foreground/70">
              Subscription
            </p>
            <HarnessRow
              embedded
              resetEpoch={resetEpoch}
              runtime={subscription}
            />
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
          />
        ) : (
          <SettingsOptionRow
            data-testid={`settings-models-${primary.providerId}-workspace`}
          >
            <div className="min-w-0">
              <p className="font-medium text-foreground">Workspace sign-in</p>
              <p
                className="mt-0.5 text-sm text-muted-foreground/70"
                data-settings-subcopy
              >
                {primary.label} uses a workspace host and OAuth sign-in,
                configured under Agent defaults in Settings › Agents.
              </p>
            </div>
          </SettingsOptionRow>
        )}
      </SettingsOptionGroup>
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
          Couldn't load Agent defaults, so keys and the default can't be changed
          here right now. Reopen Settings to try again.
        </p>
      ) : null}
      <SettingsOptionGroupList>
        {groups.map(renderGroup)}
        <DefaultForNewAgentsGroup
          disabled={busy || catalog === undefined}
          notice={noticeFor("default")}
          onChoose={handleChooseDefault}
          options={options}
        />
      </SettingsOptionGroupList>
    </div>
  );
}
