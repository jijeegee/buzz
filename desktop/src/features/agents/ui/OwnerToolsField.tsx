import * as React from "react";

import type { RespondToMode } from "@/shared/api/types";
import { useT } from "@/shared/i18n";
import { cn } from "@/shared/lib/cn";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/shared/ui/alert-dialog";
import { Switch } from "@/shared/ui/switch";

/**
 * "Owner tools: On/Off" for one agent (AI tools S1). Off while anyone can
 * talk to the agent (the relay refuses on + anyone); turning it on for an
 * allowlist asks first, since the people allowed could then act as the
 * owner. `checked` is `null` while the level is unknown.
 */
export function OwnerToolsField({
  checked,
  disabled,
  idPrefix,
  mode,
  onCheckedChange,
}: {
  checked: boolean | null;
  disabled: boolean;
  idPrefix: string;
  mode: RespondToMode;
  onCheckedChange: (value: boolean) => void;
}) {
  const t = useT();
  const [confirming, setConfirming] = React.useState(false);
  const switchId = `${idPrefix}-owner-tools`;
  const anyone = mode === "anyone";
  const unknown = checked === null;
  const on = !anyone && checked === true;
  const off = disabled || anyone || unknown;

  function change(value: boolean) {
    if (value && mode === "allowlist") {
      setConfirming(true);
      return;
    }
    onCheckedChange(value);
  }

  const hint = anyone
    ? t("agents.ownerTools.anyoneDisabled")
    : unknown
      ? t("agents.ownerTools.unknown")
      : on && mode === "allowlist"
        ? t("agents.ownerTools.allowlistWarning")
        : t("agents.ownerTools.hint");

  return (
    <div
      className="flex min-h-12 items-center justify-between gap-4 rounded-xl border border-input bg-background px-3 py-3"
      data-testid={`${idPrefix}-owner-tools-row`}
    >
      <div className={cn("min-w-0", off && "opacity-50")}>
        <label
          className="text-sm font-medium text-foreground"
          htmlFor={switchId}
        >
          {t("agents.ownerTools.label")}:{" "}
          {on ? t("agents.ownerTools.on") : t("agents.ownerTools.off")}
        </label>
        <p
          className={cn(
            "text-xs",
            on && mode === "allowlist"
              ? "text-destructive"
              : "text-muted-foreground",
          )}
        >
          {hint}
        </p>
      </div>
      <Switch
        aria-label={t("agents.ownerTools.label")}
        checked={on}
        data-testid={`${idPrefix}-owner-tools`}
        disabled={off}
        id={switchId}
        onCheckedChange={change}
      />
      <AlertDialog onOpenChange={setConfirming} open={confirming}>
        <AlertDialogContent data-testid={`${idPrefix}-owner-tools-confirm`}>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {t("agents.ownerTools.confirmTitle")}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {t("agents.ownerTools.confirmBody")}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t("common.cancel")}</AlertDialogCancel>
            <AlertDialogAction
              className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
              data-testid={`${idPrefix}-owner-tools-confirm-on`}
              onClick={() => onCheckedChange(true)}
            >
              {t("agents.ownerTools.confirmAction")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
