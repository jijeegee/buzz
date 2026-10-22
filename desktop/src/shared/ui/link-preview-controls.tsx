import { type MessageKey, useT } from "@/shared/i18n";
import { EllipsisVertical, EyeOff } from "lucide-react";
import { toast } from "sonner";

import { useAppShell } from "@/app/AppShellContext";
import {
  setLinkPreviewStyle,
  type LinkPreviewStyle,
  useLinkPreviewStyle,
} from "@/shared/lib/linkPreviewStylePreference";
import { cn } from "@/shared/lib/cn";
import { Button } from "@/shared/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "@/shared/ui/dropdown-menu";

const CONTROL_BUTTON_CLASS =
  "h-5 w-5 rounded-full text-muted-foreground opacity-0 transition-opacity hover:text-foreground focus-visible:opacity-100 group-hover/message:opacity-100 data-[state=open]:opacity-100";

const LINK_PREVIEW_STYLE_OPTIONS: {
  value: LinkPreviewStyle;
  label: MessageKey;
}[] = [
  { value: "rich", label: "ui.linkPreview.styleRich" },
  { value: "compact", label: "ui.linkPreview.styleCompact" },
];

export function LinkPreviewControls({
  onRemove,
  placement = "right",
}: {
  onRemove?: () => void;
  placement?: "left" | "right";
}) {
  const style = useLinkPreviewStyle();
  const { onOpenSettings } = useAppShell();
  const t = useT();

  const handleStyleChange = (nextStyle: string) => {
    if (
      (nextStyle !== "rich" && nextStyle !== "compact") ||
      nextStyle === style
    ) {
      return;
    }

    setLinkPreviewStyle(nextStyle);
    toast.success(
      t(
        nextStyle === "rich"
          ? "ui.linkPreview.setToRich"
          : "ui.linkPreview.setToCompact",
      ),
      {
        action: onOpenSettings
          ? {
              label: t("ui.linkPreview.appearanceAction"),
              onClick: () => onOpenSettings("appearance"),
            }
          : undefined,
        description: t("ui.linkPreview.changeInAppearance"),
      },
    );
  };

  return (
    <div
      className={cn(
        "absolute top-0 z-20 flex flex-col",
        placement === "left" ? "right-full" : "left-full ml-1",
      )}
    >
      <DropdownMenu modal={false}>
        <DropdownMenuTrigger asChild>
          <Button
            aria-label={t("ui.linkPreview.displaySettings")}
            className={CONTROL_BUTTON_CLASS}
            size="icon-xs"
            title={t("ui.linkPreview.displaySettings")}
            type="button"
            variant="ghost"
          >
            <EllipsisVertical aria-hidden="true" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" side="right">
          <DropdownMenuSub>
            <DropdownMenuSubTrigger>
              {t("ui.linkPreview.display")}
            </DropdownMenuSubTrigger>
            <DropdownMenuSubContent>
              <DropdownMenuRadioGroup
                onValueChange={handleStyleChange}
                value={style}
              >
                {LINK_PREVIEW_STYLE_OPTIONS.map((option) => (
                  <DropdownMenuRadioItem
                    key={option.value}
                    value={option.value}
                  >
                    {t(option.label)}
                  </DropdownMenuRadioItem>
                ))}
              </DropdownMenuRadioGroup>
            </DropdownMenuSubContent>
          </DropdownMenuSub>
          {onRemove ? (
            <>
              <DropdownMenuSeparator />
              <DropdownMenuItem
                className="text-destructive focus:text-destructive"
                onClick={onRemove}
              >
                <EyeOff aria-hidden="true" />
                {t("ui.linkPreview.removePreview")}
              </DropdownMenuItem>
            </>
          ) : null}
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  );
}
