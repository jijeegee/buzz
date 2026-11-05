import {
  finishAgentSetupPending,
  getAgentSetupPending,
  getGlobalAgentConfig,
} from "@/shared/api/tauriGlobalAgentConfig";
import { discoverAcpRuntimes } from "@/shared/api/tauriAcpDiscovery";
import type {
  AcpRuntimeCatalogEntry,
  GlobalAgentConfig,
} from "@/shared/api/types";
import { runtimeIsReadyForOnboarding } from "./ui/onboardingRuntimeSelection";

const SETUP_CHECK_TIMEOUT_MS = 10_000;
/** Bundled tools always look available, but run nothing until given a provider. */
const PROVIDER_RUNTIMES = new Set(["kuzz-agent", "goose"]);

/** The account's default AI tool is connected on this computer. */
export function accountIsSetUp(
  config: Pick<GlobalAgentConfig, "preferred_runtime" | "provider">,
  runtimes: readonly AcpRuntimeCatalogEntry[],
) {
  const preferred = runtimes.find(
    (runtime) => runtime.id === config.preferred_runtime,
  );
  if (!preferred || !runtimeIsReadyForOnboarding(preferred)) return false;
  return !PROVIDER_RUNTIMES.has(preferred.id) || Boolean(config.provider);
}

/**
 * Agent setup belongs to this account on this computer: settings are saved per
 * account, so another account's setup never counts. A failed or slow check
 * keeps the screens.
 */
export async function checkAccountSetUp() {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    const config = await getGlobalAgentConfig();
    if (!config.preferred_runtime) return false;
    const runtimes = await Promise.race([
      discoverAcpRuntimes({ force: true }),
      new Promise<never>((_, reject) => {
        timer = setTimeout(
          () => reject(new Error("timed out")),
          SETUP_CHECK_TIMEOUT_MS,
        );
      }),
    ]);
    return accountIsSetUp(config, runtimes);
  } catch (error) {
    console.warn("Failed to check this account's AI tools.", error);
    return false;
  } finally {
    clearTimeout(timer);
  }
}

/**
 * The one check after any sign-in, however it was reached. Plain app starts
 * have nothing pending, so putting setup off asks again at the next sign-in.
 */
export async function checkAfterSignIn(pubkey: string) {
  const pending = await getAgentSetupPending(pubkey).catch(() => false);
  if (!pending) return "none";
  if (!(await checkAccountSetUp())) return "needs-setup";
  await finishAgentSetupPending(pubkey).catch(() => {});
  return "set-up";
}
