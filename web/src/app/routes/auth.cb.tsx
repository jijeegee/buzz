import { createFileRoute } from "@tanstack/react-router";

import { AuthCallbackPage } from "@/shared/auth/ui/AuthCallbackPage";

export const Route = createFileRoute("/auth/cb")({
  component: AuthCallbackPage,
});
