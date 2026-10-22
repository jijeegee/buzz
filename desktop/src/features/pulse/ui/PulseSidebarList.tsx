import type * as React from "react";
import { useLocation, useNavigate } from "@tanstack/react-router";
import { AtSign, Bot, Globe, Heart, Search, UserCheck } from "lucide-react";

import {
  PULSE_TAB_LABELS,
  type PulseTab,
  parsePulseTab,
} from "@/features/pulse/ui/PulseView";
import { useHistorySearchState } from "@/shared/hooks/useHistorySearchState";
import { useT } from "@/shared/i18n";
import { cn } from "@/shared/lib/cn";

const TAB_KEYS = ["tab"] as const;
const ICONS: Record<PulseTab, React.ComponentType<{ className?: string }>> = {
  everyone: Globe,
  people: UserCheck,
  liked: Heart,
  agents: Bot,
  mine: AtSign,
  search: Search,
};

/** The Pulse tab's list, in the left column: one row per feed. */
export function PulseSidebarList() {
  const t = useT();
  const navigate = useNavigate();
  const pathname = useLocation({ select: (location) => location.pathname });
  const { values } = useHistorySearchState(TAB_KEYS);
  const active = pathname === "/pulse" ? parsePulseTab(values.tab) : null;
  return (
    <div className="flex min-h-0 flex-1 flex-col" data-testid="pulse-list">
      <h2 className="px-3 pb-2 pt-1 text-base font-semibold text-sidebar-foreground">
        {t("pulse.sidebar.title")}
      </h2>
      <div className="flex min-h-0 flex-1 flex-col overflow-y-auto px-1 pb-4">
        {(Object.keys(PULSE_TAB_LABELS) as PulseTab[]).map((tab) => {
          const Icon = ICONS[tab];
          return (
            <button
              aria-current={active === tab || undefined}
              className={cn(
                "flex h-10 w-full items-center gap-2.5 rounded-md px-2 text-left text-sm text-sidebar-foreground outline-hidden transition-colors hover:bg-sidebar-accent focus-visible:ring-2 focus-visible:ring-sidebar-ring",
                active === tab && "bg-sidebar-accent font-medium",
              )}
              data-testid={`pulse-list-${tab}`}
              key={tab}
              onClick={() =>
                void navigate({
                  to: "/pulse",
                  search: tab === "everyone" ? {} : { tab },
                })
              }
              type="button"
            >
              <Icon className="h-4 w-4 shrink-0 text-sidebar-foreground/60" />
              <span className="truncate">{t(PULSE_TAB_LABELS[tab])}</span>
            </button>
          );
        })}
      </div>
    </div>
  );
}
