import { AlertCircle, Check } from "lucide-react";

export type SaveNotice = {
  tone: "ok" | "error";
  text: string;
};

/** The inline save result, in the same voice as Agent defaults. */
export function SaveNoticeLine({ notice }: { notice: SaveNotice | null }) {
  if (!notice) return null;
  return notice.tone === "ok" ? (
    <span
      className="flex min-w-0 items-center gap-1 text-sm text-green-600 dark:text-green-400"
      data-testid="settings-models-save-notice"
      role="status"
    >
      <Check aria-hidden="true" className="size-3.5 shrink-0" />
      {notice.text}
    </span>
  ) : (
    <span
      className="flex min-w-0 items-center gap-1 text-sm text-destructive"
      data-testid="settings-models-save-notice"
      role="alert"
    >
      <AlertCircle aria-hidden="true" className="size-3.5 shrink-0" />
      {notice.text}
    </span>
  );
}
