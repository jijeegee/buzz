import React, { useSyncExternalStore } from "react";
import { bindAccountCache } from "@/shared/auth/account-cache";
import { webSession } from "@/shared/auth/session";
import ReactDOM from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { App } from "@/app/App";
import "@fontsource-variable/inter/wght.css";
import "@/shared/styles/globals.css";
import { ThemeProvider } from "@/shared/theme/ThemeProvider";
import { Toaster } from "@/shared/ui/sonner";
import { TooltipProvider } from "@/shared/ui/tooltip";

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      retry: 1,
      refetchOnWindowFocus: false,
      networkMode: "always",
      gcTime: 5 * 60 * 1_000,
    },
    mutations: {
      networkMode: "always",
    },
  },
});

const accountCache = bindAccountCache(webSession, queryClient);

function AccountApp() {
  const epoch = useSyncExternalStore(
    accountCache.subscribe,
    accountCache.getSnapshot,
  );
  return <App key={epoch} />;
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>
      <ThemeProvider>
        <TooltipProvider delayDuration={300}>
          <AccountApp />
          <Toaster />
        </TooltipProvider>
      </ThemeProvider>
    </QueryClientProvider>
  </React.StrictMode>,
);
