import { createRootRoute } from "@tanstack/react-router";

import { AppShell } from "@/app/AppShell";
import { HuddlePresenceProvider } from "@/features/huddle/HuddlePresenceContext";
import { ScopedPeopleFavoritesProvider } from "@/features/people/PeopleFavoritesContext";
import { UserStatusLookupProvider } from "@/features/user-status/UserStatusLookupContext";

function RootRoute() {
  return (
    <HuddlePresenceProvider>
      <UserStatusLookupProvider>
        <ScopedPeopleFavoritesProvider>
          <AppShell />
        </ScopedPeopleFavoritesProvider>
      </UserStatusLookupProvider>
    </HuddlePresenceProvider>
  );
}

export const Route = createRootRoute({
  component: RootRoute,
});
