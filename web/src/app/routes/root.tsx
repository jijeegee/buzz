import { Outlet, createRootRoute } from "@tanstack/react-router";

import { AccountControl } from "@/shared/auth/ui/AccountControl";

export const Route = createRootRoute({
  component: RootLayout,
});

function RootLayout() {
  return (
    <div className="flex min-h-dvh flex-col">
      <header className="flex justify-end px-4 py-2 empty:hidden">
        <AccountControl />
      </header>
      <main className="flex flex-1 flex-col">
        <Outlet />
      </main>
    </div>
  );
}
