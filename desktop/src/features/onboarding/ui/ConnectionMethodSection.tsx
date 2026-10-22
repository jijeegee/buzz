import type * as React from "react";
import { ChevronRight, CreditCard, KeyRound } from "lucide-react";

import { type MessageKey, useT } from "@/shared/i18n";
import { Button } from "@/shared/ui/button";
import type { HarnessConnectionMethod } from "./harnessConnectionOptions";

const CONNECTION_METHOD_CHOICES = [
  {
    icon: CreditCard,
    labelKey: "onboarding.connectionMethod.subscription",
    method: "subscription",
  },
  {
    icon: KeyRound,
    labelKey: "onboarding.connectionMethod.apiKey",
    method: "api",
  },
] as const satisfies ReadonlyArray<{
  icon: React.ComponentType<{ className?: string }>;
  labelKey: MessageKey;
  method: HarnessConnectionMethod;
}>;

export function ConnectionMethodSection({
  onSelect,
}: {
  onSelect: (method: HarnessConnectionMethod) => void;
}) {
  const t = useT();
  return (
    <section
      className="flex min-h-0 w-full flex-1 flex-col"
      data-testid="onboarding-harness-method"
    >
      <div className="shrink-0">
        <h1 className="text-title font-normal text-foreground">
          {t("onboarding.connectionMethod.title")}
        </h1>
        <p className="mt-2 text-base leading-6 text-foreground/80">
          {t("onboarding.connectionMethod.description")}
        </p>
      </div>

      <div className="mt-6 flex w-full flex-1 flex-col gap-3">
        {CONNECTION_METHOD_CHOICES.map(
          ({ icon: MethodIcon, labelKey, method }) => (
            <Button
              className="group h-auto min-h-14 w-full items-center justify-start gap-4 rounded-2xl bg-foreground/[0.04] px-4 py-3 text-left text-sm text-foreground shadow-none transition-colors duration-150 ease-out hover:bg-foreground/[0.08] hover:text-foreground focus-visible:ring-2 focus-visible:ring-foreground/20 motion-reduce:transition-none"
              data-testid={`onboarding-harness-method-${method}`}
              key={method}
              onClick={() => onSelect(method)}
              type="button"
              variant="ghost"
            >
              <span className="flex size-6 shrink-0 items-center justify-center">
                <MethodIcon aria-hidden className="!size-6" />
              </span>
              <span className="min-w-0 flex-1 font-medium">{t(labelKey)}</span>
              <span className="ml-auto flex size-8 shrink-0 items-center justify-center">
                <ChevronRight
                  aria-hidden
                  className="size-4 text-muted-foreground transition-colors duration-150 ease-out group-hover:text-foreground motion-reduce:transition-none"
                />
              </span>
            </Button>
          ),
        )}
      </div>
    </section>
  );
}
