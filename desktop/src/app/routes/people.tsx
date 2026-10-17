import * as React from "react";
import { createFileRoute } from "@tanstack/react-router";

import {
  parseProfilePanelTab,
  parseProfilePanelView,
  type ProfilePanelTab,
  type ProfilePanelView,
} from "@/features/profile/ui/UserProfilePanelUtils";
import { ViewLoadingFallback } from "@/shared/ui/ViewLoadingFallback";

const PeopleScreen = React.lazy(async () => {
  const module = await import("@/features/people/ui/PeopleScreen");
  return { default: module.PeopleScreen };
});

type PeopleRouteSearch = {
  profile?: string;
  profileTab?: ProfilePanelTab;
  profileView?: ProfilePanelView;
};

function validatePeopleSearch(
  search: Record<string, unknown>,
): PeopleRouteSearch {
  return {
    profile:
      typeof search.profile === "string" && search.profile.length > 0
        ? search.profile
        : undefined,
    profileTab: parseProfilePanelTab(search.profileTab) ?? undefined,
    profileView: parseProfilePanelView(search.profileView) ?? undefined,
  };
}

export const Route = createFileRoute("/people")({
  validateSearch: validatePeopleSearch,
  component: PeopleRouteComponent,
});

function PeopleRouteComponent() {
  return (
    <React.Suspense fallback={<ViewLoadingFallback kind="agents" />}>
      <PeopleScreen />
    </React.Suspense>
  );
}
