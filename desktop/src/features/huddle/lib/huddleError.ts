import { t } from "@/shared/i18n";
export type HuddleAction = "join" | "start";

function rawErrorMessage(error: unknown): string | null {
  if (error instanceof Error) {
    return error.message;
  }
  if (typeof error === "string") {
    return error;
  }
  return null;
}

export function formatHuddleActionError(
  error: unknown,
  action: HuddleAction,
): string {
  const message = rawErrorMessage(error)?.trim();
  const normalized = message?.toLowerCase();

  if (
    normalized?.includes("huddle_audio_unavailable") ||
    normalized?.includes("huddle audio unavailable in this deployment")
  ) {
    return t("huddle.error.audioUnavailable");
  }

  if (message) {
    return message;
  }

  return action === "join"
    ? t("huddle.error.joinFailed")
    : t("huddle.error.startFailed");
}
