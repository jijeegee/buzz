import { formatTtlDuration } from "@/features/channels/lib/ephemeralChannel";
import { t } from "@/shared/i18n";

export type ChannelLifecycle = "ongoing" | "temporary" | "project";

export function channelLifecycle(input: {
  projectHome: boolean;
  temporary: boolean;
}): ChannelLifecycle {
  if (input.projectHome) return "project";
  return input.temporary ? "temporary" : "ongoing";
}

export function channelLifecycleLabel(
  lifecycle: ChannelLifecycle,
  ttlSeconds: number | null,
): string {
  if (lifecycle === "project") return t("channels.type.project");
  if (lifecycle === "temporary" && ttlSeconds != null) {
    return t("channels.type.temporaryWithTtl", {
      ttl: formatTtlDuration(ttlSeconds),
    });
  }
  if (lifecycle === "temporary") return t("channels.type.temporary");
  return t("channels.type.ongoing");
}
