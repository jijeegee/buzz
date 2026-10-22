import type { MeshServingUsage } from "@/shared/api/tauriMesh";
import { t } from "@/shared/i18n";

/**
 * Pure projection of host-side serving usage into a small, politely-worded
 * indicator model for the Share compute card.
 *
 * Single source of truth for "who is using the compute I'm sharing" copy, so
 * the component and its tests agree. Kept pure/total (accepts null = not yet
 * fetched) and defensive (all fields optional-safe via the Rust extractor).
 *
 * Distinctions that matter:
 * - `localAttempts` = this machine's OWN agents using the local model. Not a
 *   "someone else is here" signal — surfaced softly as activity, not as a peer.
 * - `remoteAttempts` / `endpointAttempts` = another member consuming this
 *   machine's compute. THIS is the "someone connected to what I'm sharing"
 *   signal.
 */
export type MeshServingIndicator = {
  /** Whether to show anything at all (only while actively sharing). */
  show: boolean;
  /** Someone is being served right now. */
  active: boolean;
  /** A non-local member is (or has been) consuming this machine's compute. */
  hasRemoteConsumers: boolean;
  /** One-line status suitable for the card. */
  label: string;
  /** Longer detail for a tooltip / secondary line. */
  detail: string | null;
};

/**
 * @param usage  latest snapshot from `meshServingUsage`, or null if not fetched
 * @param isSharing  whether this machine is currently in serve mode (card owns
 *                   this from the toggle model). Usage is only meaningful while
 *                   sharing.
 */
export function deriveServingIndicator(
  usage: MeshServingUsage | null,
  isSharing: boolean,
): MeshServingIndicator {
  const hidden: MeshServingIndicator = {
    show: false,
    active: false,
    hasRemoteConsumers: false,
    label: "",
    detail: null,
  };
  if (!isSharing || !usage) {
    return hidden;
  }

  const hasRemoteConsumers =
    usage.remoteAttempts > 0 || usage.endpointAttempts > 0;
  const active = usage.inflight > 0;

  // Remote consumer present (or seen) — the headline case the user asked for.
  if (hasRemoteConsumers) {
    const remote = usage.remoteAttempts + usage.endpointAttempts;
    const rate = Math.round(usage.tokensPerSecond);
    const label = active
      ? t("meshCompute.usage.inUseByMember", { count: usage.inflight })
      : t("meshCompute.usage.usedByMember", { count: remote });
    const detail =
      usage.peers > 0
        ? t("meshCompute.usage.peersAndRate", { peers: usage.peers, rate })
        : t("meshCompute.usage.rate", { rate });
    return { show: true, active, hasRemoteConsumers: true, label, detail };
  }

  // Only local (this machine's own agents) — show softly as activity.
  if (active) {
    return {
      show: true,
      active: true,
      hasRemoteConsumers: false,
      label: t("meshCompute.usage.servingYourAgent", {
        count: usage.inflight,
      }),
      detail: t("meshCompute.usage.rate", {
        rate: Math.round(usage.tokensPerSecond),
      }),
    };
  }
  if (usage.requestsServed > 0) {
    return {
      show: true,
      active: false,
      hasRemoteConsumers: false,
      label: t("meshCompute.usage.idleNow"),
      detail: t("meshCompute.usage.servedThisSession", {
        count: usage.requestsServed,
      }),
    };
  }

  // Sharing but nothing served yet.
  return {
    show: true,
    active: false,
    hasRemoteConsumers: false,
    label: t("meshCompute.usage.idleYet"),
    detail: null,
  };
}
