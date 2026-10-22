import { t } from "@/shared/i18n";

const MINUTE = 60;
const HOUR = 3600;
const DAY = 86400;

export function formatRelativeTime(unixSeconds: number): string {
  const now = Math.floor(Date.now() / 1000);
  const delta = Math.max(0, now - unixSeconds);

  if (delta < MINUTE) {
    return t("time.justNow");
  }

  if (delta < HOUR) {
    const minutes = Math.floor(delta / MINUTE);
    return t("forum.time.minutesAgo", { minutes });
  }

  if (delta < DAY) {
    const hours = Math.floor(delta / HOUR);
    return t("forum.time.hoursAgo", { hours });
  }

  const days = Math.floor(delta / DAY);
  return t("forum.time.daysAgo", { days });
}
