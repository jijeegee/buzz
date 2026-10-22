import {
  Camera,
  Circle,
  GalleryThumbnails,
  Palette,
  UserRound,
} from "lucide-react";

import { type MessageKey, useT } from "@/shared/i18n";
import { cn } from "@/shared/lib/cn";

export type ReviewSection = "person" | "shape" | "color" | "poster";

const REVIEW_SECTIONS: {
  key: ReviewSection;
  label: MessageKey;
  caption: MessageKey;
  hidden?: boolean;
  icon: typeof UserRound;
}[] = [
  {
    caption: "profile.reviewNav.personCaption",
    icon: UserRound,
    key: "person",
    label: "profile.reviewNav.personLabel",
  },
  {
    caption: "profile.reviewNav.shapeCaption",
    hidden: true,
    icon: Circle,
    key: "shape",
    label: "profile.reviewNav.shapeLabel",
  },
  {
    caption: "profile.reviewNav.colorCaption",
    icon: Palette,
    key: "color",
    label: "profile.reviewNav.colorLabel",
  },
  {
    caption: "profile.reviewNav.posterCaption",
    icon: GalleryThumbnails,
    key: "poster",
    label: "profile.reviewNav.posterLabel",
  },
];

type AnimatedAvatarReviewNavProps = {
  activeSection: ReviewSection;
  disabled?: boolean;
  isSaving: boolean;
  onRetake: () => void;
  onSectionChange: (section: ReviewSection) => void;
  testIdPrefix: string;
};

export function AnimatedAvatarReviewNav({
  activeSection,
  disabled = false,
  isSaving,
  onRetake,
  onSectionChange,
  testIdPrefix,
}: AnimatedAvatarReviewNavProps) {
  const t = useT();
  const controlsDisabled = disabled || isSaving;

  return (
    <div
      className="flex items-start justify-center gap-7"
      data-testid={`${testIdPrefix}-animated-sections`}
    >
      {REVIEW_SECTIONS.filter((section) => !section.hidden).map((section) => {
        const Icon = section.icon;
        return (
          <button
            aria-label={t(section.label)}
            aria-pressed={activeSection === section.key}
            className="group flex flex-col items-center gap-1.5 rounded-xl focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
            data-testid={`${testIdPrefix}-animated-section-${section.key}`}
            disabled={controlsDisabled}
            key={section.key}
            onClick={() => onSectionChange(section.key)}
            title={t(section.label)}
            type="button"
          >
            <span
              className={cn(
                "grid h-12 w-12 place-items-center rounded-full transition-[background-color,color,transform] duration-150 ease-[cubic-bezier(0.23,1,0.32,1)] motion-reduce:transition-none motion-safe:group-hover:scale-[1.04] motion-safe:group-active:scale-[0.98] group-disabled:scale-100",
                activeSection === section.key
                  ? "bg-foreground text-background"
                  : "bg-muted text-muted-foreground/70 group-hover:bg-muted/80 group-hover:text-muted-foreground group-disabled:bg-muted group-disabled:text-muted-foreground/70",
              )}
            >
              <Icon
                aria-hidden="true"
                className="h-5 w-5 transition-transform duration-150 ease-[cubic-bezier(0.23,1,0.32,1)] motion-reduce:transition-none motion-safe:group-hover:rotate-[5deg] motion-safe:group-hover:scale-[1.12] motion-safe:group-active:scale-[0.98]"
              />
            </span>
            <span
              className={cn(
                "text-sm transition-colors duration-150 ease-out",
                activeSection === section.key
                  ? "text-foreground"
                  : "text-muted-foreground",
              )}
            >
              {t(section.caption)}
            </span>
          </button>
        );
      })}
      <span
        aria-hidden="true"
        className="h-12 w-px shrink-0 rounded-full bg-border/70"
      />
      <button
        aria-label={t("profile.reviewNav.retakeLabel")}
        className="group flex flex-col items-center gap-1.5 rounded-xl focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
        data-testid={`${testIdPrefix}-animated-retake`}
        disabled={controlsDisabled}
        key="retake"
        onClick={onRetake}
        title={t("profile.reviewNav.retakeLabel")}
        type="button"
      >
        <span className="grid h-12 w-12 place-items-center rounded-full bg-muted text-muted-foreground/70 transition-[background-color,color,transform] duration-150 ease-[cubic-bezier(0.23,1,0.32,1)] motion-reduce:transition-none group-hover:bg-muted/80 group-hover:text-muted-foreground motion-safe:group-hover:scale-[1.04] motion-safe:group-active:scale-[0.98] group-disabled:bg-muted group-disabled:text-muted-foreground/70 group-disabled:scale-100">
          <Camera
            aria-hidden="true"
            className="h-5 w-5 transition-transform duration-150 ease-[cubic-bezier(0.23,1,0.32,1)] motion-reduce:transition-none motion-safe:group-hover:rotate-[5deg] motion-safe:group-hover:scale-[1.12] motion-safe:group-active:scale-[0.98]"
          />
        </span>
        <span className="text-sm text-muted-foreground">
          {t("profile.reviewNav.retake")}
        </span>
      </button>
    </div>
  );
}
