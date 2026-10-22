import * as React from "react";

import {
  useAvailableAcpRuntimes,
  useCreateChannelManagedAgentMutation,
} from "@/features/agents/hooks";
import { resolvePersonaRuntime } from "@/features/agents/lib/resolvePersonaRuntime";
import type { AgentPersona } from "@/shared/api/types";
import { t } from "@/shared/i18n";

type QuickBotDropState = {
  pending: boolean;
  error: string | null;
};

/**
 * Handles creating a new managed agent from a persona with a given instance name.
 */
export function useQuickBotDrop(channelId: string | null) {
  const createMutation = useCreateChannelManagedAgentMutation(channelId);
  const providersQuery = useAvailableAcpRuntimes();
  const [state, setState] = React.useState<QuickBotDropState>({
    pending: false,
    error: null,
  });

  const providers = providersQuery.data ?? [];
  const defaultProvider = providers[0] ?? null;

  const addBot = React.useCallback(
    async (persona: AgentPersona, instanceName: string) => {
      if (state.pending || !channelId) return;

      setState({ pending: true, error: null });

      try {
        const { runtime } = resolvePersonaRuntime(
          persona.runtime,
          providers,
          defaultProvider,
        );

        if (!runtime) {
          setState({
            pending: false,
            error: t("channels.quickBots.noRuntime"),
          });
          return;
        }

        await createMutation.mutateAsync({
          runtime,
          name: instanceName,
          systemPrompt: persona.systemPrompt,
          avatarUrl: persona.avatarUrl ?? undefined,
          personaId: persona.id,
          model: persona.model ?? undefined,
        });

        setState({ pending: false, error: null });
      } catch (err) {
        setState({
          pending: false,
          error:
            err instanceof Error
              ? err.message
              : t("channels.quickBots.createFailed"),
        });
      }
    },
    [channelId, createMutation, defaultProvider, providers, state.pending],
  );

  return { ...state, addBot };
}
