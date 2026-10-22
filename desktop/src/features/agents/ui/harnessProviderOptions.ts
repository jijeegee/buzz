import type { HarnessProviderInfo } from "@/shared/api/types";
import type { PersonaModelOption } from "./agentConfigOptions";

/**
 * Harness-owned providers (Hermes): the providers the user set up inside the
 * harness. Kuzz never changes them; it only lets an agent pick which of them
 * (and which of its models) to run. No model means the harness default.
 */

/** Description Desktop puts on a saved model the harness no longer offers. */
export const HARNESS_UNAVAILABLE_MODEL_DESCRIPTION =
  "Not available in Hermes — the agent runs the Hermes default";

/** Description Desktop puts on a saved model whose provider is not set up in Hermes. */
export const HARNESS_NOT_SET_UP_MODEL_DESCRIPTION =
  "Its provider is not set up in Hermes — pick another model";

export function defaultHarnessProvider(
  providers: readonly HarnessProviderInfo[],
): HarnessProviderInfo | undefined {
  return providers.find((provider) => provider.isDefault) ?? providers[0];
}

/**
 * The provider a model id belongs to: the longest provider id that prefixes
 * `<id>:`. Ids carry extra colons (`custom:<name>:<model>`, `llama3:8b`), so a
 * plain split would be wrong. No model, or a model of no listed provider (a
 * saved model the harness no longer offers), belongs to the default provider.
 */
export function harnessProviderOfModel(
  model: string,
  providers: readonly HarnessProviderInfo[],
): string | undefined {
  const trimmed = model.trim();
  const match = providers
    .filter((provider) => trimmed.startsWith(`${provider.id}:`))
    .sort((left, right) => right.id.length - left.id.length)[0];
  return (match ?? defaultHarnessProvider(providers))?.id;
}

/**
 * The model options for one provider. The "default model" row (empty id)
 * means the harness default, which only the default provider runs.
 */
export function modelOptionsForHarnessProvider(
  options: readonly PersonaModelOption[],
  providers: readonly HarnessProviderInfo[],
  providerId: string | undefined,
): readonly PersonaModelOption[] {
  const defaultId = defaultHarnessProvider(providers)?.id;
  return options.filter((option) => {
    if (option.id.trim().length === 0) {
      return providerId === defaultId;
    }
    return harnessProviderOfModel(option.id, providers) === providerId;
  });
}

/**
 * The model to store when the user picks a provider: unset (the harness
 * default) for the default provider, otherwise that provider's first model.
 */
export function modelForHarnessProvider(
  providerId: string,
  options: readonly PersonaModelOption[],
  providers: readonly HarnessProviderInfo[],
): string {
  if (providerId === defaultHarnessProvider(providers)?.id) {
    return "";
  }
  return (
    options.find(
      (option) =>
        option.id.trim().length > 0 &&
        harnessProviderOfModel(option.id, providers) === providerId,
    )?.id ?? ""
  );
}
